//! Candidates for reusing decoded payload blocks from local storage.
//!
//! Providers index block hashes; the fetcher verifies every candidate before use.

use std::path::Path;

use byte_calc::NumBytes;

/// Locate candidate decoded blocks by their hash.
///
/// The payload's hash algorithm and chunker determine the index to query.
pub trait StoredBlockProvider {
    /// Return a candidate block with this decoded hash, if one is indexed.
    ///
    /// Unreadable or mismatching candidates fall back to the original bundle bytes.
    fn query(&self, hash: &[u8]) -> Option<StoredBlock<'_>>;
}

/// Location and decoded length of one locally indexed block candidate.
#[derive(Debug, Clone, Copy)]
pub struct StoredBlock<'provider> {
    /// File containing the block.
    pub file: &'provider Path,
    /// Offset of the block in the file.
    pub offset: NumBytes,
    /// Size of the block in the file.
    pub size: NumBytes,
}
