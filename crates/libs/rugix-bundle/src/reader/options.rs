//! Range acquisition limits shared by framing reads and payload workers.

use std::num::NonZeroUsize;

use reportify::bail;

use crate::BundleResult;

/// Limits for consuming a bundle through a range source.
///
/// Forward streams remain sequential. With range access, retained responses use
/// at most `(concurrency + 1) * max_range_size` bytes, including the separate
/// fallback response. Metadata, block buffers, and codec workspace are additional.
#[derive(Debug, Clone, Copy)]
pub struct ReaderOptions {
    /// Maximum number of range workers and regular requests in flight.
    ///
    /// Defaults to four. A fallback request shares these workers and has its own
    /// response slot.
    pub concurrency: NonZeroUsize,
    /// Maximum encoded bytes per range, including framing and fallback reads.
    ///
    /// Defaults to 1 MiB. Larger encoded blocks are assembled from several ranges.
    /// The value must fit within `isize::MAX` for in-memory response buffers.
    pub max_range_size: NonZeroUsize,
}

impl Default for ReaderOptions {
    fn default() -> Self {
        Self {
            concurrency: NonZeroUsize::new(4).expect("the default concurrency is nonzero"),
            max_range_size: NonZeroUsize::new(1024 * 1024)
                .expect("the default range size is nonzero"),
        }
    }
}

impl ReaderOptions {
    /// Check that response buffers fit the platform's allocation size.
    pub(crate) fn validate(&self) -> BundleResult<()> {
        if self.max_range_size.get() > isize::MAX as usize {
            bail!("maximum range size exceeds the platform's buffer size limit");
        }
        Ok(())
    }
}
