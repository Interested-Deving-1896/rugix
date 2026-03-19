use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use reportify::ResultExt;
use tracing::info;

use crate::apps::orchestrator::{AppContext, AppStatus, Orchestrator};
use crate::apps::AppsResult;

/// Manages a single executable via a service manager.
pub struct Binary;

/// Fixed name of the systemd unit template in the generation directory.
const UNIT_TEMPLATE: &str = "systemd.service";

impl Binary {
    fn require_systemd(ctx: &AppContext) -> AppsResult<()> {
        if ctx.service_manager != "systemd" {
            reportify::bail!(
                "binary orchestrator requires service-manager \"systemd\", got \"{}\"",
                ctx.service_manager
            );
        }
        Ok(())
    }

    /// Derive the systemd service name from the app name.
    fn service_name(app_name: &str) -> String {
        format!("rugix-app-{app_name}.service")
    }

    /// Directory that holds the rendered units for the active generation.
    fn persistent_unit_dir(ctx: &AppContext) -> PathBuf {
        ctx.generation_dir.join(".rugix/systemd/units")
    }

    /// Runtime path where systemd can pick up the unit immediately.
    fn runtime_unit_path(app_name: &str) -> PathBuf {
        Path::new("/run/systemd/system").join(Self::service_name(app_name))
    }

    /// Read the unit template and substitute placeholders.
    fn render_unit(ctx: &AppContext) -> AppsResult<String> {
        let template_path = ctx.generation_dir.join(UNIT_TEMPLATE);
        let template =
            fs::read_to_string(&template_path).whatever("unable to read unit template")?;
        let rendered = template
            .replace("${GENERATION_DIR}", &ctx.generation_dir.to_string_lossy())
            .replace("${DATA_DIR}", &ctx.data_dir.to_string_lossy());
        Ok(rendered)
    }

    /// Reload Systemd.
    fn daemon_reload() -> AppsResult<()> {
        let status = Command::new("systemctl")
            .arg("daemon-reload")
            .status()
            .whatever("unable to run systemctl daemon-reload")?;
        if !status.success() {
            reportify::bail!("systemctl daemon-reload failed");
        }
        Ok(())
    }
}

impl Orchestrator for Binary {
    fn name(&self) -> &str {
        "binary"
    }

    fn activate(&self, ctx: &AppContext) -> AppsResult<()> {
        Self::require_systemd(ctx)?;

        let unit_content = Self::render_unit(ctx)?;
        let service = Self::service_name(ctx.app_name);

        let persist_dir = Self::persistent_unit_dir(ctx);
        fs::create_dir_all(&persist_dir).whatever("unable to create units directory")?;
        let persist_path = persist_dir.join(&service);
        fs::write(&persist_path, &unit_content).whatever("unable to write persistent unit file")?;
        info!(app = ctx.app_name, unit = ?persist_path, "persisted unit");

        let runtime_path = Self::runtime_unit_path(ctx.app_name);
        fs::write(&runtime_path, &unit_content).whatever("unable to write runtime unit file")?;
        info!(app = ctx.app_name, unit = ?runtime_path, "installed runtime unit");

        Self::daemon_reload()?;

        // Enable with --runtime to create .wants/ symlinks under
        // /run/systemd/system/, integrating the unit into systemd's boot
        // dependency graph.  This is restored by restore-units after reboot.
        let _ = Command::new("systemctl")
            .args(["enable", "--runtime", &service])
            .status();

        let start_status = Command::new("systemctl")
            .args(["start", &service])
            .status()
            .whatever("unable to run systemctl start")?;
        if !start_status.success() {
            reportify::bail!("systemctl start {service} failed");
        }
        info!(app = ctx.app_name, service = %service, "enabled and started");

        Ok(())
    }

    fn status(&self, ctx: &AppContext) -> AppsResult<AppStatus> {
        Self::require_systemd(ctx)?;
        let service = Self::service_name(ctx.app_name);
        let output = Command::new("systemctl")
            .arg("is-active")
            .arg(&service)
            .output()
            .whatever("unable to run systemctl is-active")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        match stdout.trim() {
            "active" => Ok(AppStatus::Running),
            "inactive" | "dead" => Ok(AppStatus::Stopped),
            "failed" => Ok(AppStatus::Failed {
                message: "unit failed".to_owned(),
            }),
            _ => Ok(AppStatus::Unknown),
        }
    }

    fn deactivate(&self, ctx: &AppContext) -> AppsResult<()> {
        Self::require_systemd(ctx)?;

        let service = Self::service_name(ctx.app_name);

        let _ = Command::new("systemctl").args(["stop", &service]).status();
        let _ = Command::new("systemctl")
            .args(["disable", "--runtime", &service])
            .status();
        info!(app = ctx.app_name, service = %service, "stopped and disabled");

        let persist_dir = Self::persistent_unit_dir(ctx);
        let persist_path = persist_dir.join(&service);
        if persist_path.exists() {
            info!(app = ctx.app_name, unit = ?persist_path, "removing persistent unit");
            let _ = fs::remove_file(&persist_path);
        }

        let runtime_path = Self::runtime_unit_path(ctx.app_name);
        if runtime_path.exists() {
            info!(app = ctx.app_name, unit = ?runtime_path, "removing runtime unit");
            let _ = fs::remove_file(&runtime_path);
            let _ = Self::daemon_reload();
        }
        Ok(())
    }

    fn start(&self, ctx: &AppContext) -> AppsResult<()> {
        Self::require_systemd(ctx)?;
        let service = Self::service_name(ctx.app_name);
        let status = Command::new("systemctl")
            .args(["start", &service])
            .status()
            .whatever("unable to run systemctl start")?;
        if !status.success() {
            reportify::bail!("systemctl start {service} failed");
        }
        info!(app = ctx.app_name, service = %service, "started");
        Ok(())
    }

    fn stop(&self, ctx: &AppContext) -> AppsResult<()> {
        Self::require_systemd(ctx)?;
        let service = Self::service_name(ctx.app_name);
        let status = Command::new("systemctl")
            .args(["stop", &service])
            .status()
            .whatever("unable to run systemctl stop")?;
        if !status.success() {
            reportify::bail!("systemctl stop {service} failed");
        }
        info!(app = ctx.app_name, service = %service, "stopped");
        Ok(())
    }
}
