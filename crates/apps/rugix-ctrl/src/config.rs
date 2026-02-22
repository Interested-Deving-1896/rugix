mod generated;
use std::fs;
use std::path::Path;

// Re-export the generated data structures.
pub use generated::*;
use reportify::ResultExt;

use crate::system::SystemResult;

/// Ctrl config path.
const CTRL_CONFIG_PATH: &str = "/etc/rugix/ctrl.toml";

pub fn load_ctrl_config() -> SystemResult<config::Config> {
    Ok(if Path::new(CTRL_CONFIG_PATH).exists() {
        toml::from_str(
            &fs::read_to_string(CTRL_CONFIG_PATH).whatever("unable to read configuration file")?,
        )
        .whatever("unable to parse configuration file")?
    } else {
        config::Config::default()
    })
}
