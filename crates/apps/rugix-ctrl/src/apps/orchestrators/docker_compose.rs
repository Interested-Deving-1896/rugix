use std::fs;
use std::process::Command;

use reportify::ResultExt;
use tracing::info;

use crate::apps::orchestrator::{AppContext, AppStatus, Orchestrator};
use crate::apps::AppsResult;

/// Docker Compose orchestrator.
pub struct DockerCompose;

impl DockerCompose {
    /// Build a `docker compose` command with the right project name and file flags.
    fn compose_cmd(ctx: &AppContext) -> Command {
        let mut cmd = Command::new("docker");
        cmd.arg("compose");
        cmd.arg("--project-name").arg(ctx.app_name);
        cmd.arg("-f")
            .arg(ctx.generation_dir.join("docker-compose.yml"));
        cmd
    }
}

impl Orchestrator for DockerCompose {
    fn name(&self) -> &str {
        "docker-compose"
    }

    fn activate(&self, ctx: &AppContext) -> AppsResult<()> {
        // Load all image tarballs from the images/ subdirectory.
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

        // Start the containers.
        info!(app = ctx.app_name, "starting docker compose");
        let status = Self::compose_cmd(ctx)
            .arg("up")
            .arg("-d")
            .arg("--remove-orphans")
            .status()
            .whatever("unable to run docker compose up")?;
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
        Ok(AppStatus::Running)
    }

    fn deactivate(&self, ctx: &AppContext) -> AppsResult<()> {
        info!(app = ctx.app_name, "stopping docker compose");
        let _ = Self::compose_cmd(ctx).arg("down").status();
        Ok(())
    }
}
