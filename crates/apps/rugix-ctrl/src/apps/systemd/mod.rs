//! Systemd integration for Rugix Apps.

use std::process::Command;

use reportify::ResultExt;

use super::AppsResult;

/// Runtime directory where systemd picks up transient units.
pub const RUNTIME_UNITS_DIR: &str = "/run/systemd/system";

pub mod restore;

/// Run `systemctl daemon-reload`.
pub fn daemon_reload() -> AppsResult<()> {
    run(&["daemon-reload"])
}

/// Enable a unit with `--runtime` (best-effort, ignores failures).
pub fn enable_runtime(unit: &str) {
    let _ = run(&["enable", "--runtime", unit]);
}

/// Disable a unit with `--runtime` (best-effort, ignores failures).
pub fn disable_runtime(unit: &str) {
    let _ = run(&["disable", "--runtime", unit]);
}

/// Start a unit, waiting for it to finish.
pub fn start(unit: &str) -> AppsResult<()> {
    run(&["start", unit])
}

/// Queue a unit for start without blocking.
pub fn start_no_block(unit: &str) -> AppsResult<()> {
    run(&["start", "--no-block", unit])
}

/// Stop a unit.
pub fn stop(unit: &str) -> AppsResult<()> {
    run(&["stop", unit])
}

/// Check whether a unit is active.
///
/// Returns the raw status string (e.g. `"active"`, `"inactive"`).
pub fn is_active(unit: &str) -> AppsResult<String> {
    let output = Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .whatever("unable to run systemctl is-active")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Run `systemctl` with the given arguments.
fn run(args: &[&str]) -> AppsResult<()> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .whatever(format!("unable to run systemctl {}", args.join(" ")))?;
    if !status.success() {
        reportify::bail!("systemctl {} failed", args.join(" "));
    }
    Ok(())
}
