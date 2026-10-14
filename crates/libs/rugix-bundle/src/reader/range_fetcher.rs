//! Bounded lookahead and prioritized fallback reads using blocking worker threads.

use std::collections::VecDeque;
use std::io::Read;
use std::ops::Range;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::thread::JoinHandle;

use flume::Receiver;
use flume::Sender;
use reportify::ResultExt;
use reportify::bail;
use tracing::error;
use tracing::trace;

use crate::BundleError;
use crate::BundleResult;
use crate::reader::RangeSource;
use crate::reader::ReaderOptions;
use reportify::Report;

/// Fetch ordered intervals while allowing a rejected block to request any earlier
/// interval.
pub(crate) struct RangeFetcher {
    pool: WorkerPool,
    options: ReaderOptions,
    ranges: VecDeque<Range<u64>>,
    pending: VecDeque<PendingRange>,
}

impl RangeFetcher {
    pub(crate) fn new(
        source: Arc<dyn RangeSource>,
        ranges: Vec<Range<u64>>,
        options: ReaderOptions,
    ) -> BundleResult<Self> {
        let mut fetcher = Self {
            pool: WorkerPool::new(source, ranges.len().clamp(1, options.concurrency.get()))?,
            options,
            ranges: ranges.into(),
            pending: VecDeque::new(),
        };
        fetcher.fill_window()?;
        Ok(fetcher)
    }

    /// Consume a block from the already scheduled, ordered missing-byte intervals.
    pub(crate) fn read(&mut self, range: Range<u64>) -> BundleResult<Vec<u8>> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(
                usize::try_from(range.end - range.start)
                    .whatever("encoded block does not fit into memory")?,
            )
            .whatever("unable to allocate encoded block")?;
        let mut position = range.start;
        while position < range.end {
            let pending = self
                .pending
                .front_mut()
                .ok_or_else(|| reportify::whatever!("missing scheduled bundle range"))?;
            if position < pending.range.start || position >= pending.range.end {
                bail!("block lies outside the next scheduled range");
            }
            if pending.data.is_none() {
                pending.data = Some(self.pool.receive(&pending.receiver)?);
            }
            let end = range.end.min(pending.range.end);
            let start_idx = (position - pending.range.start) as usize;
            let end_idx = (end - pending.range.start) as usize;
            output.extend_from_slice(
                &pending
                    .data
                    .as_ref()
                    .expect("the pending range was received before reading")
                    [start_idx..end_idx],
            );
            position = end;
            if position == pending.range.end {
                self.pending.pop_front();
                self.fill_window()?;
            }
        }
        Ok(output)
    }

    /// Reserve a separate response slot and prioritize a block needed by the consumer.
    pub(crate) fn fallback(&self, range: Range<u64>) -> BundleResult<Vec<u8>> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(
                usize::try_from(range.end - range.start)
                    .whatever("encoded block does not fit into memory")?,
            )
            .whatever("unable to allocate fallback block")?;
        let mut start = range.start;
        while start < range.end {
            let end = start
                .saturating_add(self.options.max_range_size.get() as u64)
                .min(range.end);
            let pending = self.pool.submit(start..end, true)?;
            let data = self.pool.receive(&pending.receiver)?;
            output.extend_from_slice(&data);
            start = end;
        }
        Ok(output)
    }

    /// All planned data has been consumed; subsequent framing reads remain usable.
    pub(crate) fn finish(&mut self) {
        self.pool.completed = true;
    }

    fn fill_window(&mut self) -> BundleResult<()> {
        while self.pending.len() < self.options.concurrency.get() {
            let Some(range) = self.ranges.pop_front() else {
                break;
            };
            self.pending.push_back(self.pool.submit(range, false)?);
        }
        Ok(())
    }
}

struct PendingRange {
    range: Range<u64>,
    receiver: Receiver<Result<Vec<u8>, ()>>,
    data: Option<Vec<u8>>,
}

struct Job {
    range: Range<u64>,
    result: Sender<Result<Vec<u8>, ()>>,
}

#[derive(Default)]
struct Jobs {
    regular: VecDeque<Job>,
    urgent: VecDeque<Job>,
    stopping: bool,
}

#[derive(Default)]
struct WorkerState {
    jobs: Mutex<Jobs>,
    changed: Condvar,
    failure: Mutex<Option<Report<BundleError>>>,
}

/// Each job has its own result slot, so later results cannot block an earlier result.
struct WorkerPool {
    source: Arc<dyn RangeSource>,
    state: Arc<WorkerState>,
    workers: Vec<JoinHandle<()>>,
    completed: bool,
}

impl WorkerPool {
    fn new(source: Arc<dyn RangeSource>, count: usize) -> BundleResult<Self> {
        let mut pool = Self {
            source,
            state: Arc::new(WorkerState::default()),
            workers: Vec::new(),
            completed: false,
        };
        for index in 0..count {
            let source = Arc::clone(&pool.source);
            let state = Arc::clone(&pool.state);
            pool.workers.push(
                std::thread::Builder::new()
                    .name(format!("bundle-fetch-{index}"))
                    .spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            worker(Arc::clone(&source), Arc::clone(&state));
                        }));
                        if result.is_err() {
                            fail(
                                &*source,
                                &state,
                                reportify::whatever!("bundle fetch worker panicked"),
                            );
                        }
                    })
                    .whatever("unable to start bundle fetch worker")?,
            );
        }
        Ok(pool)
    }

    fn submit(&self, range: Range<u64>, urgent: bool) -> BundleResult<PendingRange> {
        let (result, receiver) = flume::bounded(1);
        let job = Job {
            range: range.clone(),
            result,
        };
        let mut jobs = self
            .state
            .jobs
            .lock()
            .expect("the job queue is only locked around infallible queue operations");
        if jobs.stopping {
            return Err(self.failure());
        }
        if urgent {
            jobs.urgent.push_back(job)
        } else {
            jobs.regular.push_back(job)
        }
        self.state.changed.notify_one();
        Ok(PendingRange {
            range,
            receiver,
            data: None,
        })
    }

    fn receive(&self, receiver: &Receiver<Result<Vec<u8>, ()>>) -> BundleResult<Vec<u8>> {
        match receiver.recv() {
            Ok(Ok(data)) => Ok(data),
            Ok(Err(())) | Err(_) => Err(self.failure()),
        }
    }

    fn failure(&self) -> Report<BundleError> {
        self.state
            .failure
            .lock()
            .expect("failure lock only protects moving a report")
            .take()
            .unwrap_or_else(|| reportify::whatever!("bundle fetch worker stopped unexpectedly"))
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        {
            let mut jobs = self
                .state
                .jobs
                .lock()
                .expect("the job queue is only locked around infallible queue operations");
            jobs.stopping = true;
            jobs.regular.clear();
            jobs.urgent.clear();
            self.state.changed.notify_all();
        }
        if !self.completed {
            self.source.cancel();
        }
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                error!("bundle fetch worker panicked");
            }
        }
    }
}

fn worker(source: Arc<dyn RangeSource>, state: Arc<WorkerState>) {
    loop {
        let job = {
            let mut jobs = state
                .jobs
                .lock()
                .expect("the job queue is only locked around infallible queue operations");
            loop {
                if jobs.stopping {
                    return;
                }
                if let Some(job) = jobs.urgent.pop_front().or_else(|| jobs.regular.pop_front()) {
                    break job;
                }
                jobs = state
                    .changed
                    .wait(jobs)
                    .expect("the job queue is only locked around infallible queue operations");
            }
        };
        let result = match fetch_range(&*source, &state, job.range) {
            Ok(data) => Ok(data),
            Err(error) => {
                fail(&*source, &state, error);
                Err(())
            }
        };
        if job.result.send(result).is_err() {
            trace!("range result receiver closed during shutdown");
        }
    }
}

fn fetch_range(
    source: &dyn RangeSource,
    state: &WorkerState,
    range: Range<u64>,
) -> BundleResult<Vec<u8>> {
    let mut reader = source.open_range(range.clone())?;
    let mut buffer = vec![0; (range.end - range.start) as usize];
    let mut position = 0;
    while position < buffer.len() {
        if state
            .jobs
            .lock()
            .expect("the job queue is only locked around infallible queue operations")
            .stopping
        {
            bail!("bundle fetching canceled");
        }
        let end = (position + 16 * 1024).min(buffer.len());
        match reader.read(&mut buffer[position..end]) {
            Ok(0) => bail!("unexpected end of bundle range"),
            Ok(read) => position += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).whatever("unable to read bundle range"),
        }
    }
    Ok(buffer)
}

/// Preserve the original failure before waking consumers and canceling other readers.
fn fail(source: &dyn RangeSource, state: &WorkerState, error: Report<BundleError>) {
    {
        let mut failure = state
            .failure
            .lock()
            .expect("failure lock only protects moving a report");
        if failure.is_none() {
            *failure = Some(error)
        }
    }
    {
        let mut jobs = state
            .jobs
            .lock()
            .expect("the job queue is only locked around infallible queue operations");
        jobs.stopping = true;
        jobs.regular.clear();
        jobs.urgent.clear();
        state.changed.notify_all();
    }
    source.cancel();
}
