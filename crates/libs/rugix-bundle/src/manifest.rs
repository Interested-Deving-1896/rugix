mod generated;
pub use generated::manifest::*;

use crate::BundleResult;

/// Validate that an app name is safe for use in file paths, systemd unit names, and
/// Docker project names.
///
/// Allowed characters: ASCII lowercase letters, digits, hyphens, and underscores.
///
/// Names must not be empty, must not start with a hyphen or digit.
pub fn validate_app_name(name: &str) -> BundleResult<()> {
    if name.is_empty() {
        reportify::bail!("app name must not be empty");
    }
    if name.starts_with('-') {
        reportify::bail!("app name must not start with a hyphen: {name:?}");
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        reportify::bail!("app name must not start with a digit: {name:?}");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        reportify::bail!(
            "app name contains invalid characters (allowed: a-z, 0-9, hyphen, underscore): {name:?}"
        );
    }
    Ok(())
}
