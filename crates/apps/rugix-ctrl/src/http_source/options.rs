//! HTTP installation settings passed from the CLI through the operation protocol.

use std::num::NonZeroUsize;
use std::time::Duration;

use byte_calc::NumBytes;
use rugix_bundle::reader::ReaderOptions;
use serde::Deserialize;
use serde::Serialize;

/// HTTP fetch settings independent of retry policy and connection deadlines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpFetchConfig {
    /// Maximum regular range requests in flight.
    pub concurrency: NonZeroUsize,
    /// Maximum bytes per range request.
    pub max_range_size: NonZeroUsize,
    /// Maximum idle wait for incoming socket data.
    pub stall_timeout: Duration,
}

impl Default for HttpFetchConfig {
    fn default() -> Self {
        let reader = ReaderOptions::default();
        Self {
            concurrency: reader.concurrency,
            max_range_size: reader.max_range_size,
            stall_timeout: Duration::from_secs(60),
        }
    }
}

impl HttpFetchConfig {
    /// Pass transport-independent acquisition limits to the bundle reader.
    pub(crate) fn reader_options(&self) -> ReaderOptions {
        ReaderOptions {
            concurrency: self.concurrency,
            max_range_size: self.max_range_size,
        }
    }
}

/// Parse a positive byte size that fits a response buffer, accepting units such as MiB.
pub(crate) fn parse_max_range_size(value: &str) -> Result<NonZeroUsize, String> {
    let bytes = value
        .parse::<NumBytes>()
        .map_err(|error| error.to_string())?;
    let size = usize::try_from(bytes.raw)
        .ok()
        .and_then(NonZeroUsize::new)
        .filter(|size| size.get() <= isize::MAX as usize)
        .ok_or_else(|| {
            "range size must be positive and fit the platform's buffer size limit".to_owned()
        })?;
    Ok(size)
}
