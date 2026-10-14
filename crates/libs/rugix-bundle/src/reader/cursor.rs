//! Private parser state over an owned bundle input, shared with payload acquisition.

use std::io::BufReader;
use std::io::Read;
use std::num::NonZeroUsize;
use std::sync::Arc;

use byte_calc::NumBytes;
use reportify::ResultExt;
use reportify::bail;

use crate::BundleResult;
use crate::format::stlv::Read as StlvRead;
use crate::reader::BundleSource;
use crate::reader::RangeSource;
use crate::reader::SourceReader;

use super::BundleProgress;

/// Parser position and buffered metadata reader, owned exclusively by BundleReader.
pub(crate) struct Cursor {
    range_source: Option<Arc<dyn RangeSource>>,
    reader: Option<BufReader<SourceReader>>,
    reader_end: u64,
    position: u64,
    sequential_bytes: u64,
    max_range_size: u64,
    total_bytes: Option<NumBytes>,
}

impl Cursor {
    pub(crate) fn new(source: BundleSource, max_range_size: NonZeroUsize) -> Self {
        let (range_source, reader) = match source {
            BundleSource::StreamSource(reader) => (None, Some(BufReader::new(reader))),
            BundleSource::RangeSource(source) => (Some(source), None),
        };
        let total_bytes = range_source.as_ref().map(|source| source.size().into());
        Self {
            range_source,
            reader,
            reader_end: 0,
            position: 0,
            sequential_bytes: 0,
            max_range_size: max_range_size.get() as u64,
            total_bytes,
        }
    }

    pub(crate) fn set_size_hint(&mut self, total: Option<NumBytes>) {
        self.total_bytes = self.total_bytes.or(total);
    }

    pub(crate) fn progress(&self) -> BundleProgress {
        BundleProgress {
            bytes_read: self.position.into(),
            bytes_total: self.total_bytes,
        }
    }

    pub(crate) fn range_source(&self) -> Option<Arc<dyn RangeSource>> {
        self.range_source.as_ref().map(Arc::clone)
    }

    /// Resume framing at the end of the independently consumed payload.
    pub(crate) fn advance_to(&mut self, position: u64) -> BundleResult<()> {
        let remaining = position
            .checked_sub(self.position)
            .ok_or_else(|| reportify::whatever!("bundle parser cannot move backwards"))?;
        self.skip(remaining.into())
    }

    /// Keep prefetching sequential payload data across bounded ranges.
    pub(crate) fn hint_sequential_bytes(&mut self, length: NumBytes) {
        self.sequential_bytes = length.raw;
    }
}

impl StlvRead for Cursor {
    fn read(&mut self, buffer: &mut [u8]) -> BundleResult<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if let Some(source) = &self.range_source {
            if self.position == source.size() {
                return Ok(0);
            }
            if self.reader.is_none() || self.position == self.reader_end {
                let length = self
                    .sequential_bytes
                    .max(buffer.len() as u64)
                    .max(8192)
                    .min(self.max_range_size);
                let end = self.position.saturating_add(length).min(source.size());
                self.reader_end = end;
                self.reader = Some(BufReader::new(source.open_range(self.position..end)?));
            }
        }
        let reader = self
            .reader
            .as_mut()
            .expect("stream inputs have a reader and ranged inputs open one before reading");
        let read = Read::read(reader, buffer).whatever("unable to read bundle framing")?;
        self.position += read as u64;
        self.sequential_bytes = self.sequential_bytes.saturating_sub(read as u64);
        Ok(read)
    }

    fn skip(&mut self, length: NumBytes) -> BundleResult<()> {
        if let Some(source) = &self.range_source {
            let position = self
                .position
                .checked_add(length.raw)
                .ok_or_else(|| reportify::whatever!("bundle cursor position overflow"))?;
            if position > source.size() {
                bail!("skip exceeds bundle size");
            }
            self.position = position;
            self.reader = None;
            self.sequential_bytes = 0;
            return Ok(());
        }
        let mut remaining = length.raw;
        let mut buffer = [0; 8192];
        while remaining > 0 {
            let chunk = remaining.min(buffer.len() as u64) as usize;
            let read = StlvRead::read(self, &mut buffer[..chunk])?;
            if read == 0 {
                bail!("unexpected end of bundle while skipping");
            }
            remaining -= read as u64;
        }
        Ok(())
    }
}
