//! Orchestrator for managing Docker Compose stacks.

use std::fmt::Write;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use reportify::ResultExt;
use tracing::{info, warn};

use super::{AppContext, AppStatus, AppStatusMessage, Orchestrator};
use crate::apps::AppsResult;

/// Name of the env file written into the generation directory.
const ENV_FILE: &str = "rugix-app.env";

/// Default timeout (in seconds) for `docker compose up --wait`.
const DEFAULT_HEALTH_CHECK_TIMEOUT: u64 = 120;

/// Number of log lines per service to include when Compose activation fails.
const DIAGNOSTIC_LOG_TAIL: usize = 120;

/// Maximum number of bytes to include in activation failure diagnostics.
const MAX_DIAGNOSTICS_BYTES: usize = 64 * 1024;

/// Docker Compose orchestrator.
pub struct DockerCompose;

impl DockerCompose {
    /// Write the Rugix environment file into the generation directory.
    fn write_env_file(ctx: &AppContext) -> AppsResult<()> {
        let mut content = String::new();
        writeln!(content, "RUGIX_APP_NAME={}", ctx.app_name).unwrap();
        writeln!(content, "RUGIX_APP_DIR={}", ctx.app_dir.display()).unwrap();
        writeln!(
            content,
            "RUGIX_APP_GENERATION_DIR={}",
            ctx.generation_dir.display()
        )
        .unwrap();
        writeln!(content, "RUGIX_APP_DATA_DIR={}", ctx.data_dir.display()).unwrap();
        let env_path = ctx.generation_dir.join(ENV_FILE);
        fs::write(&env_path, content).whatever("unable to write rugix-app.env")?;
        Ok(())
    }

    /// Build a `docker compose` command with the right project name, file, and env file.
    fn compose_cmd(ctx: &AppContext) -> Command {
        let mut cmd = Command::new("docker");
        cmd.arg("compose");
        cmd.arg("--project-name").arg(ctx.app_name);
        cmd.arg("-f")
            .arg(ctx.generation_dir.join("docker-compose.yml"));
        let env_path = ctx.generation_dir.join(ENV_FILE);
        if env_path.exists() {
            cmd.arg("--env-file").arg(env_path);
        }
        cmd
    }

    /// Get the health check timeout from the manifest, falling back to the default.
    fn health_check_timeout(ctx: &AppContext) -> u64 {
        ctx.manifest
            .health_check
            .as_ref()
            .and_then(|hc| hc.timeout)
            .unwrap_or(DEFAULT_HEALTH_CHECK_TIMEOUT)
    }

    /// Collect best-effort diagnostics while failed containers still exist.
    fn activation_diagnostics(ctx: &AppContext) -> String {
        let mut diagnostics = String::new();
        let _ = writeln!(
            diagnostics,
            "docker compose diagnostics for app {}:",
            ctx.app_name
        );
        let _ = writeln!(diagnostics);

        Self::append_compose_output(
            ctx,
            &mut diagnostics,
            "docker compose ps --all",
            &["ps", "--all"],
        );
        let _ = writeln!(diagnostics);
        let log_tail = DIAGNOSTIC_LOG_TAIL.to_string();
        Self::append_compose_output(
            ctx,
            &mut diagnostics,
            "docker compose logs --no-color --timestamps --tail {DIAGNOSTIC_LOG_TAIL}",
            &["logs", "--no-color", "--timestamps", "--tail", &log_tail],
        );

        truncate_diagnostics(diagnostics)
    }

    fn append_compose_output(
        ctx: &AppContext,
        diagnostics: &mut String,
        label: &str,
        args: &[&str],
    ) {
        let _ = writeln!(diagnostics, "### {label}");
        match Self::compose_cmd(ctx).args(args).output() {
            Ok(output) => {
                if !output.status.success() {
                    let _ = writeln!(diagnostics, "command exited with {}", output.status);
                }
                append_command_output(diagnostics, &output.stdout, &output.stderr);
            }
            Err(err) => {
                let _ = writeln!(diagnostics, "unable to run diagnostics command: {err}");
            }
        }
    }

    fn write_activation_diagnostics(ctx: &AppContext, diagnostics: &str) -> Option<PathBuf> {
        let path = ctx
            .generation_dir
            .join(".rugix")
            .join("activation-diagnostics.log");
        let parent = path.parent().expect("diagnostics path has parent");
        if let Err(err) = fs::create_dir_all(parent) {
            warn!(
                app = ctx.app_name,
                path = ?path,
                "unable to create diagnostics directory: {err}"
            );
            return None;
        }
        if let Err(err) = fs::write(&path, diagnostics) {
            warn!(
                app = ctx.app_name,
                path = ?path,
                "unable to write activation diagnostics: {err}"
            );
            return None;
        }
        info!(
            app = ctx.app_name,
            path = ?path,
            "wrote activation diagnostics"
        );
        Some(path)
    }
}

impl Orchestrator for DockerCompose {
    fn name(&self) -> &str {
        "docker-compose"
    }

    fn activate(&self, ctx: &AppContext) -> AppsResult<()> {
        Self::write_env_file(ctx)?;

        let images_dir = ctx.generation_dir.join("images");
        if images_dir.exists() {
            let entries = fs::read_dir(&images_dir).whatever("unable to read images directory")?;
            for entry in entries {
                let entry = entry.whatever("unable to read directory entry")?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("tar") {
                    info!(image = ?path, "loading docker image");
                    let status = Command::new("docker")
                        .arg("image")
                        .arg("load")
                        .arg("-i")
                        .arg(&path)
                        .status()
                        .whatever("unable to run docker image load")?;
                    if !status.success() {
                        reportify::bail!("docker image load failed for {}", path.display());
                    }
                }
            }
        }

        info!(app = ctx.app_name, "starting docker compose");
        let mut cmd = Self::compose_cmd(ctx);
        cmd.arg("up").arg("-d").arg("--remove-orphans");

        let timeout = Self::health_check_timeout(ctx);
        if timeout > 0 {
            cmd.arg("--wait")
                .arg("--wait-timeout")
                .arg(timeout.to_string());
        }

        let status = cmd.status().whatever("unable to run docker compose up")?;
        if !status.success() {
            let diagnostics = Self::activation_diagnostics(ctx);
            if let Some(path) = Self::write_activation_diagnostics(ctx, &diagnostics) {
                reportify::bail!(
                    "docker compose up failed; diagnostics written to {}\n\n{diagnostics}",
                    path.display()
                );
            }
            reportify::bail!("docker compose up failed\n\n{diagnostics}");
        }
        Ok(())
    }

    fn status(&self, ctx: &AppContext) -> AppsResult<AppStatus> {
        let output = Self::compose_cmd(ctx)
            .arg("ps")
            .arg("--format")
            .arg("json")
            .output()
            .whatever("unable to run docker compose ps")?;
        if !output.status.success() {
            return Ok(AppStatus::Unknown);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Ok(AppStatus::Stopped);
        }
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(container) = serde_json::from_str::<ContainerStatus>(line) else {
                continue;
            };
            if !container.state.eq_ignore_ascii_case("running") {
                return Ok(AppStatus::Failed(AppStatusMessage::new(format!(
                    "container {} is {}",
                    container.name.as_deref().unwrap_or("unknown"),
                    container.state
                ))));
            }
            if let Some(health) = &container.health {
                if health.eq_ignore_ascii_case("unhealthy") {
                    return Ok(AppStatus::Unhealthy(AppStatusMessage::new(format!(
                        "container {} is unhealthy",
                        container.name.as_deref().unwrap_or("unknown"),
                    ))));
                }
            }
        }
        Ok(AppStatus::Running)
    }

    fn deactivate(&self, ctx: &AppContext) -> AppsResult<()> {
        info!(app = ctx.app_name, "stopping docker compose");
        match Self::compose_cmd(ctx).arg("down").status() {
            Ok(status) if !status.success() => {
                warn!(
                    app = ctx.app_name,
                    "docker compose down failed (best-effort)"
                );
            }
            Err(err) => {
                warn!(
                    app = ctx.app_name,
                    "unable to run docker compose down: {err} (best-effort)"
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn start(&self, ctx: &AppContext) -> AppsResult<()> {
        Self::write_env_file(ctx)?;
        info!(app = ctx.app_name, "starting docker compose");
        let mut cmd = Self::compose_cmd(ctx);
        cmd.arg("up").arg("-d");

        let timeout = Self::health_check_timeout(ctx);
        if timeout > 0 {
            cmd.arg("--wait")
                .arg("--wait-timeout")
                .arg(timeout.to_string());
        }

        let status = cmd.status().whatever("unable to run docker compose up")?;
        if !status.success() {
            let diagnostics = Self::activation_diagnostics(ctx);
            if let Some(path) = Self::write_activation_diagnostics(ctx, &diagnostics) {
                reportify::bail!(
                    "docker compose up failed; diagnostics written to {}\n\n{diagnostics}",
                    path.display()
                );
            }
            reportify::bail!("docker compose up failed\n\n{diagnostics}");
        }
        Ok(())
    }

    fn stop(&self, ctx: &AppContext) -> AppsResult<()> {
        info!(app = ctx.app_name, "stopping docker compose containers");
        let status = Self::compose_cmd(ctx)
            .arg("stop")
            .status()
            .whatever("unable to run docker compose stop")?;
        if !status.success() {
            reportify::bail!("docker compose stop failed");
        }
        Ok(())
    }
}

/// A single container entry from `docker compose ps --format json`.
#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerStatus {
    name: Option<String>,
    state: String,
    health: Option<String>,
}

fn append_command_output(output: &mut String, stdout: &[u8], stderr: &[u8]) {
    if stdout.is_empty() && stderr.is_empty() {
        let _ = writeln!(output, "<no output>");
        return;
    }
    for bytes in [stdout, stderr] {
        if bytes.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(bytes);
        output.push_str(&text);
        if !text.ends_with('\n') {
            output.push('\n');
        }
    }
}

fn truncate_diagnostics(mut diagnostics: String) -> String {
    if diagnostics.len() <= MAX_DIAGNOSTICS_BYTES {
        return diagnostics;
    }
    let mut boundary = MAX_DIAGNOSTICS_BYTES;
    while !diagnostics.is_char_boundary(boundary) {
        boundary -= 1;
    }
    diagnostics.truncate(boundary);
    diagnostics.push_str("\n... diagnostics truncated ...\n");
    diagnostics
}
