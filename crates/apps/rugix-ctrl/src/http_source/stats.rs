//! Actual body traffic and distinct bundle-byte coverage across concurrent readers.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use byte_calc::NumBytes;

/// Body traffic can exceed the bundle size after retries or overlapping metadata reads.
#[derive(Debug, Clone)]
pub struct DownloadStats {
    pub bytes_read: NumBytes,
    pub bytes_skipped: NumBytes,
    pub total_bytes: NumBytes,
}

impl DownloadStats {
    pub fn total_bytes(&self) -> NumBytes {
        self.total_bytes
    }

    pub fn download_ratio(&self) -> f64 {
        if self.total_bytes.raw == 0 {
            return 0.0;
        }
        self.bytes_read.raw as f64 / self.total_bytes.raw as f64
    }
}

#[derive(Default)]
pub(crate) struct Counters {
    pub bytes_read: AtomicU64,
    coverage: Mutex<BTreeMap<u64, u64>>,
}

impl Counters {
    /// Merge one reader's consumed prefix when it closes, including partial retries.
    pub(crate) fn record(&self, range: Range<u64>) {
        if range.is_empty() {
            return;
        }
        let mut coverage = self
            .coverage
            .lock()
            .expect("coverage is only locked around infallible interval operations");
        let mut start = range.start;
        let mut end = range.end;
        if let Some((&previous_start, &previous_end)) = coverage.range(..=start).next_back() {
            if previous_end >= start {
                start = previous_start;
                end = end.max(previous_end);
                coverage.remove(&previous_start);
            }
        }
        while let Some((&next_start, &next_end)) = coverage.range(start..).next() {
            if next_start > end {
                break;
            }
            end = end.max(next_end);
            coverage.remove(&next_start);
        }
        coverage.insert(start, end);
    }

    pub(crate) fn snapshot(&self, total: Option<u64>) -> DownloadStats {
        let coverage = self
            .coverage
            .lock()
            .expect("coverage is only locked around infallible interval operations");
        let distinct = coverage.iter().map(|(start, end)| end - start).sum::<u64>();
        let total =
            total.unwrap_or_else(|| coverage.last_key_value().map(|(_, end)| *end).unwrap_or(0));
        DownloadStats {
            bytes_read: self.bytes_read.load(Ordering::Relaxed).into(),
            bytes_skipped: total.saturating_sub(distinct).into(),
            total_bytes: total.into(),
        }
    }
}
