use std::fs;
use std::path::Path;

use reportify::ResultExt;

pub use crate::config::apps::AppsConfig;

use super::AppsResult;

/// Apps config path.
const APPS_CONFIG_PATH: &str = "/etc/rugix/apps.toml";

/// Load the apps configuration from `/etc/rugix/apps.toml`.
///
/// Returns default values if the file does not exist.
pub fn load_apps_config() -> AppsResult<AppsConfig> {
    let path = Path::new(APPS_CONFIG_PATH);
    if !path.exists() {
        return Ok(AppsConfig::default());
    }
    let content = fs::read_to_string(path).whatever("unable to read apps config")?;
    toml::from_str(&content).whatever("unable to parse apps config")
}

/// Get the effective service manager name.
///
/// If explicitly configured, use that value.  Otherwise, attempt auto-detection
/// by checking for well-known init system indicators.  Falls back to `"none"`.
pub fn effective_service_manager(config: &AppsConfig) -> String {
    if let Some(sm) = config.service_manager.as_deref() {
        return sm.to_owned();
    }
    detect_service_manager()
}

/// Auto-detect the service manager by probing the system.
fn detect_service_manager() -> String {
    // systemd: check for the runtime directory it always creates.
    if Path::new("/run/systemd/system").is_dir() {
        return "systemd".to_owned();
    }
    "none".to_owned()
}

/// Root directory for app data.
///
/// Uses the Rugix state directory when available (so that a factory reset
/// also clears installed apps and their data), otherwise falls back to
/// `/var/lib/rugix/apps`.
pub fn apps_dir() -> &'static Path {
    const STATE_PATH: &str = "/run/rugix/state/apps";
    const VAR_PATH: &str = "/var/lib/rugix/apps";
    if Path::new("/run/rugix/state").exists() {
        Path::new(STATE_PATH)
    } else {
        Path::new(VAR_PATH)
    }
}
