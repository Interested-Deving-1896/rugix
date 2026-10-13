//! Sequential input operations for STLV framing and metadata decoding.

use std::io;

use byte_calc::NumBytes;
use reportify::ResultExt;
use reportify::bail;

use crate::BundleResult;

/// Read STLV bytes and skip values without exposing the input's capabilities.
///
/// Ordinary Rust readers implement this interface automatically. Bundle framing
/// supplies its own implementation to advance ranges without downloading them.
pub trait Read {
    /// Read bytes, returning zero at the end of the input.
    fn read(&mut self, buffer: &mut [u8]) -> BundleResult<usize>;

    /// Read exactly the requested number of bytes.
    fn read_exact(&mut self, mut buffer: &mut [u8]) -> BundleResult<()> {
        while !buffer.is_empty() {
            let read = self.read(buffer)?;
            if read == 0 {
                bail!("unexpected end of STLV input");
            }
            buffer = &mut buffer[read..];
        }
        Ok(())
    }

    /// Discard bytes of a value. Positional readers can advance without reading.
    fn skip(&mut self, length: NumBytes) -> BundleResult<()> {
        let mut remaining = length.raw;
        let mut buffer = [0; 8192];
        while remaining > 0 {
            let chunk = remaining.min(buffer.len() as u64) as usize;
            let read = self.read(&mut buffer[..chunk])?;
            if read == 0 {
                bail!("unexpected end of STLV input while skipping");
            }
            remaining -= read as u64;
        }
        Ok(())
    }
}

impl<R: io::Read + ?Sized> Read for R {
    fn read(&mut self, buffer: &mut [u8]) -> BundleResult<usize> {
        io::Read::read(self, buffer).whatever("unable to read STLV input")
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> BundleResult<()> {
        io::Read::read_exact(self, buffer).whatever("unable to read STLV input")
    }
}

/// STLV reader that skips large values by seeking instead of draining them.
///
/// Buffer the supplied reader when parsing small atom headers. Its current
/// position is preserved and its length is captured when constructing the adapter.
pub struct SeekReader<R> {
    reader: R,
    end: u64,
}

impl<R: io::Read + io::Seek> SeekReader<R> {
    pub fn new(mut reader: R) -> BundleResult<Self> {
        let position = reader
            .stream_position()
            .whatever("unable to determine STLV position")?;
        let end = reader
            .seek(io::SeekFrom::End(0))
            .whatever("unable to determine STLV length")?;
        reader
            .seek(io::SeekFrom::Start(position))
            .whatever("unable to restore STLV position")?;
        Ok(Self { reader, end })
    }

    /// Recover the original reader, including any buffered but unconsumed bytes.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

impl<R: io::Read + io::Seek> Read for SeekReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> BundleResult<usize> {
        io::Read::read(&mut self.reader, buffer).whatever("unable to read STLV input")
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> BundleResult<()> {
        io::Read::read_exact(&mut self.reader, buffer).whatever("unable to read STLV input")
    }

    fn skip(&mut self, length: NumBytes) -> BundleResult<()> {
        let position = self
            .reader
            .stream_position()
            .whatever("unable to determine STLV position")?
            .checked_add(length.raw)
            .ok_or_else(|| reportify::whatever!("STLV position overflow"))?;
        if position > self.end {
            bail!("skip exceeds STLV input length");
        }
        self.reader
            .seek(io::SeekFrom::Start(position))
            .whatever("unable to skip STLV value")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::io::BufReader;
    use std::io::Seek;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use byte_calc::NumBytes;

    use super::Read;
    use super::SeekReader;
    use crate::format::stlv::AtomHead;
    use crate::format::stlv::Tag;
    use crate::format::stlv::pretty_print;
    use crate::format::stlv::write_atom_head;
    use crate::format::stlv::write_segment_end;
    use crate::format::stlv::write_segment_start;

    struct CountedFile {
        file: std::fs::File,
        read: Arc<AtomicUsize>,
    }

    impl io::Read for CountedFile {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = io::Read::read(&mut self.file, buffer)?;
            self.read.fetch_add(read, Ordering::Relaxed);
            Ok(read)
        }
    }

    impl io::Seek for CountedFile {
        fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
            self.file.seek(position)
        }
    }

    /// Structure printing skips a large sparse value with bounded reads and rejects
    /// past-EOF skips.
    #[test]
    fn structure_printing_seeks_over_large_values() {
        let mut file = tempfile::tempfile().unwrap();
        let tag = Tag::from_bytes([1, 2, 3, 4]);
        write_segment_start(&mut file, tag).unwrap();
        let length = NumBytes::mebibytes(128);
        write_atom_head(&mut file, AtomHead::Value { tag, length }).unwrap();
        let data = file.stream_position().unwrap();
        file.seek(io::SeekFrom::Start(data + length.raw)).unwrap();
        write_segment_end(&mut file, tag).unwrap();
        file.flush().unwrap();
        file.rewind().unwrap();
        let read = Arc::new(AtomicUsize::new(0));
        let mut reader = SeekReader::new(BufReader::new(CountedFile {
            file,
            read: Arc::clone(&read),
        }))
        .unwrap();
        pretty_print(&mut reader, None).unwrap();
        assert!(read.load(Ordering::Relaxed) <= 16 * 1024);
        assert!(reader.skip(NumBytes::new(1)).is_err());
    }
}
