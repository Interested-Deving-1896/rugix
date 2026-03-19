//! Sync pre-rendered app unit files into the systemd runtime directory.
//!
//! Orchestrators persist rendered unit files in the app directory under
//! `<app_dir>/systemd/units/`. This module copies them into `/run/systemd/system/` so
//! systemd can manage them, then triggers a `daemon-reload`.
//!
//! Only apps in the `active`, `starting`, or `stopping` state are synced.  Apps that are
//! inactive, switching, or in an error state are skipped.
//!
//! Intended to be called from a oneshot service (`rugix-app-sync.service`) that runs
//! early in boot, after the data partition is mounted.

use std::fs;
use std::path::Path;
use std::process::Command;

use reportify::ResultExt;
use tracing::{error, info};

use super::manager::AppState;
use super::AppsResult;

use super::config;

/// Runtime directory where systemd picks up transient units.
const RUNTIME_UNITS_DIR: &str = "/run/systemd/system";

/// Read the persisted app state for a given app directory.
fn read_app_state(app_dir: &Path) -> AppState {
    let state_path = app_dir.join(".rugix/state.json");
    let Ok(content) = fs::read_to_string(&state_path) else {
        return AppState::Inactive;
    };
    serde_json::from_str(&content).unwrap_or(AppState::Inactive)
}

/// Sync all persisted app units into the systemd runtime directory.
///
/// Only syncs units for apps that are in the `active`, `starting`, or `stopping` state.
pub fn sync_units() -> AppsResult<()> {
    let apps_dir = config::apps_dir();
    if !apps_dir.exists() {
        info!("no apps directory, nothing to sync");
        return Ok(());
    }
    let runtime_dir = Path::new(RUNTIME_UNITS_DIR);
    let mut synced_units: Vec<String> = Vec::new();
    let entries = fs::read_dir(apps_dir).whatever("unable to read apps directory")?;
    for entry in entries {
        let entry = entry.whatever("unable to read directory entry")?;
        if !entry
            .file_type()
            .whatever("unable to get file type")?
            .is_dir()
        {
            continue;
        }

        // Only sync units for active apps (including transient starting/stopping states).
        if !matches!(
            read_app_state(&entry.path()),
            AppState::Active { .. } | AppState::Starting { .. } | AppState::Stopping { .. }
        ) {
            continue;
        }

        let unit_dir = entry.path().join("systemd/units");
        if !unit_dir.is_dir() {
            continue;
        }
        let units = fs::read_dir(&unit_dir).whatever("unable to read units directory")?;
        for unit_entry in units {
            let unit_entry = unit_entry.whatever("unable to read unit entry")?;
            let unit_path = unit_entry.path();
            let Some(file_name) = unit_path.file_name() else {
                continue;
            };
            let dest = runtime_dir.join(file_name);
            fs::copy(&unit_path, &dest).whatever("unable to copy unit file")?;
            if let Some(name) = file_name.to_str() {
                synced_units.push(name.to_owned());
            }
            info!(unit = ?file_name, "synced app unit");
        }
    }
    if !synced_units.is_empty() {
        let status = Command::new("systemctl")
            .arg("daemon-reload")
            .status()
            .whatever("unable to run systemctl daemon-reload")?;
        if !status.success() {
            reportify::bail!("systemctl daemon-reload failed");
        }
        info!(
            count = synced_units.len(),
            "daemon-reload after syncing app units"
        );
        // Start each synced unit individually. A single unit failing should
        // not prevent the remaining units from starting.
        for unit in &synced_units {
            let result = Command::new("systemctl").args(["start", unit]).status();
            match result {
                Ok(status) if status.success() => {
                    info!(unit, "started synced app unit");
                }
                Ok(status) => {
                    error!(unit, code = ?status.code(), "failed to start synced app unit");
                }
                Err(err) => {
                    error!(unit, %err, "failed to run systemctl start for synced app unit");
                }
            }
        }
    }
    Ok(())
}
