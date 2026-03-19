//! Restore app systemd units after reboot.
//!
//! Since `/run/` is a tmpfs, systemd units installed there do not survive reboots.
//! This module restores them by copying persisted unit files from the generation
//! directory (`<gen_dir>/.rugix/systemd/units/`) into `/run/systemd/system/`,
//! enabling them with `systemctl enable --runtime`, and queuing them for start with
//! `systemctl start --no-block`.
//!
//! Only apps in the `active`, `starting`, or `stopping` state are restored.  Apps
//! that are inactive, switching, or in an error state are skipped.
//!
//! Intended to be called from a oneshot service that runs early in boot, after the
//! data partition is mounted.

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

/// Extract the active generation number from the app state, if any.
fn active_generation(state: &AppState) -> Option<u64> {
    match state {
        AppState::Active { generation }
        | AppState::Starting { generation }
        | AppState::Stopping { generation } => Some(*generation),
        _ => None,
    }
}

/// Restore all persisted app units into the systemd runtime directory.
///
/// Only restores units for apps that are in the `active`, `starting`, or `stopping`
/// state.  After copying the unit files and reloading systemd, each unit is enabled
/// with `--runtime` (for bookkeeping) and queued for start with
/// `systemctl start --no-block` (which lets systemd resolve declared dependencies
/// asynchronously).
pub fn restore_units() -> AppsResult<()> {
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

        let app_dir = entry.path();
        let state = read_app_state(&app_dir);
        let Some(generation) = active_generation(&state) else {
            continue;
        };

        let unit_dir = app_dir
            .join("generations")
            .join(generation.to_string())
            .join(".rugix/systemd/units");
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
        // For each synced unit:
        //
        // 1. `enable --runtime` creates .wants/ symlinks under /run/systemd/system/ so `systemctl
        //    is-enabled` reports the correct state.
        //
        // 2. `start --no-block` queues a start job and returns immediately. Systemd resolves the
        //    unit's declared dependencies (After=, Requires=, etc.) and starts it at the right
        //    time.  Using --no-block avoids blocking the sync service (and the rest of boot) on
        //    units whose dependencies are not yet met.
        //
        // We cannot rely on enable --runtime alone because systemd computes
        // each target's job transaction before any of its dependencies run.
        // By the time the sync service creates the .wants/ symlinks, the
        // transaction is already set.
        for unit in &synced_units {
            // Enable (best-effort — missing [Install] is not fatal).
            let _ = Command::new("systemctl")
                .args(["enable", "--runtime", unit])
                .status();

            // Queue start job (non-blocking).
            let result = Command::new("systemctl")
                .args(["start", "--no-block", unit])
                .status();
            match result {
                Ok(status) if status.success() => {
                    info!(unit, "queued start for synced app unit");
                }
                Ok(status) => {
                    error!(unit, code = ?status.code(), "failed to queue start for synced app unit");
                }
                Err(err) => {
                    error!(unit, %err, "failed to run systemctl start for synced app unit");
                }
            }
        }
    }
    Ok(())
}
