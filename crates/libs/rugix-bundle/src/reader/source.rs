//! Owned bundle inputs and independent readers for positional access.
//!
//! `BundleSource` separates reusable range access from a single forward stream.

use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::ops::Range;
use std::sync::Arc;
use std::sync::Mutex;

use reportify::ResultExt;
use reportify::bail;

use crate::BundleResult;

/// An owned reader whose position belongs to one request.
pub type SourceReader = Box<dyn Read + Send>;

/// Input capabilities of a bundle installation.
pub enum BundleSource {
    /// A shareable resource that can open several independent readers.
    RangeSource(Arc<dyn RangeSource>),
    /// A single forward-only input.
    StreamSource(SourceReader),
}

/// A resource with a fixed size and independently readable byte ranges.
pub trait RangeSource: Send + Sync {
    /// Size of the selected representation.
    fn size(&self) -> u64;

    /// Open a reader bounded to the half-open interval, independent of other readers.
    ///
    /// The reader owns its resources and must report premature EOF as an error.
    /// Implementations must support cancellation or bound blocking I/O so fetch
    /// workers can finish and join after installation fails.
    fn open_range(&self, range: Range<u64>) -> BundleResult<SourceReader>;

    /// Stop pending reads after an installation failure, when the backend supports it.
    ///
    /// A canceled source need not accept further requests.
    fn cancel(&self) {}
}

impl BundleSource {
    /// Adapt a seekable input while keeping every range reader's position independent.
    pub fn from_seekable<R: Read + Seek + Send + 'static>(mut reader: R) -> BundleResult<Self> {
        let size = reader
            .seek(SeekFrom::End(0))
            .whatever("unable to determine bundle size")?;
        Ok(Self::RangeSource(Arc::new(SeekSource {
            reader: Arc::new(Mutex::new(reader)),
            size,
        })))
    }
}

/// Positional access for generic seekable inputs; seek/read pairs are serialized.
struct SeekSource<R> {
    reader: Arc<Mutex<R>>,
    size: u64,
}

impl<R: Read + Seek + Send + 'static> RangeSource for SeekSource<R> {
    fn size(&self) -> u64 {
        self.size
    }

    fn open_range(&self, range: Range<u64>) -> BundleResult<SourceReader> {
        if range.start > range.end || range.end > self.size {
            bail!("bundle range is outside the source");
        }
        Ok(Box::new(SeekRangeReader {
            reader: Arc::clone(&self.reader),
            position: range.start,
            end: range.end,
        }))
    }
}

/// Read one interval without depending on the underlying input's shared seek position.
struct SeekRangeReader<R> {
    reader: Arc<Mutex<R>>,
    position: u64,
    end: u64,
}

impl<R: Read + Seek> Read for SeekRangeReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let length = (self.end - self.position).min(buffer.len() as u64) as usize;
        if length == 0 {
            return Ok(0);
        }
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| std::io::Error::other("bundle input lock poisoned"))?;
        reader.seek(SeekFrom::Start(self.position))?;
        let read = reader.read(&mut buffer[..length])?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.position += read as u64;
        Ok(read)
    }
}
