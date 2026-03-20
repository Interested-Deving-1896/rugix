//! Orchestrator for managing Docker Compose stacks.

use std::fmt::Write;
use std::fs;
use std::process::Command;

use reportify::ResultExt;
use tracing::info;

use super::{AppContext, AppStatus, AppStatusMessage, Orchestrator};
use crate::apps::AppsResult;

/// Name of the env file written into the generation directory.
const ENV_FILE: &str = "rugix-app.env";

/// Default timeout (in seconds) for `docker compose up --wait`.
const DEFAULT_HEALTH_CHECK_TIMEOUT: u64 = 120;

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
            reportify::bail!("docker compose up failed");
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
        let _ = Self::compose_cmd(ctx).arg("down").status();
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
            reportify::bail!("docker compose up failed");
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
