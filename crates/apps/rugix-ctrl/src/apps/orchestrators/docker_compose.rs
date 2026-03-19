use std::fmt::Write;
use std::fs;
use std::process::Command;

use reportify::ResultExt;
use tracing::info;

use crate::apps::orchestrator::{AppContext, AppStatus, Orchestrator};
use crate::apps::AppsResult;

/// Name of the env file written into the generation directory.
const ENV_FILE: &str = "rugix-app.env";

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
}

impl Orchestrator for DockerCompose {
    fn name(&self) -> &str {
        "docker-compose"
    }

    fn activate(&self, ctx: &AppContext) -> AppsResult<()> {
        // Write the env file so compose can reference Rugix variables.
        Self::write_env_file(ctx)?;

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

    fn start(&self, ctx: &AppContext) -> AppsResult<()> {
        Self::write_env_file(ctx)?;
        info!(app = ctx.app_name, "starting docker compose");
        let status = Self::compose_cmd(ctx)
            .arg("up")
            .arg("-d")
            .status()
            .whatever("unable to run docker compose up")?;
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
