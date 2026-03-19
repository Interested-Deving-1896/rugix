use std::process::Command;

use reportify::ResultExt;
use tracing::info;

use crate::apps::orchestrator::{AppContext, AppStatus, Orchestrator};
use crate::apps::AppsResult;

/// Generic orchestrator delegating lifecycle operations to an `orchestrator` script.
///
/// The script is invoked with the operation name as the first argument (`activate`,
/// `status`, `deactivate`, `start`, `stop`).
///
/// The following environment variables are set:
///
/// - `RUGIX_APP_NAME` — the app name.
/// - `RUGIX_APP_DIR` — absolute path to the app directory.
/// - `RUGIX_APP_GENERATION_DIR` — absolute path to the generation directory.
/// - `RUGIX_APP_DATA_DIR` — absolute path to the app's persistent data directory.
/// - `RUGIX_APP_RECOVERY` — `"true"` if replaying an interrupted transition.
///
/// For all operations except `status`, a zero exit code means success and non-zero
/// means failure (stderr is included in the error message).
///
/// The `status` operation must print a JSON object to stdout:
///
/// ```json
/// {"status": "running"}
/// {"status": "unhealthy", "message": "health check failing"}
/// {"status": "stopped"}
/// {"status": "failed", "message": "process crashed"}
/// {"status": "unknown"}
/// ```
pub struct Generic;

/// JSON structure for the status response.
#[derive(serde::Deserialize)]
struct StatusResponse {
    status: String,
    #[serde(default)]
    message: Option<String>,
}

impl Generic {
    fn handler_path(ctx: &AppContext) -> std::path::PathBuf {
        ctx.generation_dir.join("orchestrator")
    }

    fn run_orchestrator(ctx: &AppContext, operation: &str) -> AppsResult<std::process::Output> {
        let handler = Self::handler_path(ctx);
        let output = Command::new(&handler)
            .arg(operation)
            .env("RUGIX_APP_NAME", ctx.app_name)
            .env("RUGIX_APP_DIR", ctx.app_dir)
            .env("RUGIX_APP_GENERATION_DIR", ctx.generation_dir)
            .env("RUGIX_APP_DATA_DIR", ctx.data_dir)
            .env(
                "RUGIX_APP_RECOVERY",
                if ctx.recovery { "true" } else { "false" },
            )
            .current_dir(ctx.generation_dir)
            .output()
            .whatever("unable to run handler")?;
        Ok(output)
    }

    fn run_orchestrator_checked(ctx: &AppContext, operation: &str) -> AppsResult<()> {
        let output = Self::run_orchestrator(ctx, operation)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            reportify::bail!(
                "handler `{operation}` failed (exit {}): {stderr}",
                output.status
            );
        }
        Ok(())
    }
}

impl Orchestrator for Generic {
    fn name(&self) -> &str {
        "generic"
    }

    fn activate(&self, ctx: &AppContext) -> AppsResult<()> {
        info!(app = ctx.app_name, "running generic orchestrator: activate");
        Self::run_orchestrator_checked(ctx, "activate")
    }

    fn status(&self, ctx: &AppContext) -> AppsResult<AppStatus> {
        let output = Self::run_orchestrator(ctx, "status")?;
        if !output.status.success() {
            return Ok(AppStatus::Unknown);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let Ok(response) = serde_json::from_str::<StatusResponse>(stdout.trim()) else {
            return Ok(AppStatus::Unknown);
        };
        match response.status.as_str() {
            "running" => Ok(AppStatus::Running),
            "unhealthy" => Ok(AppStatus::Unhealthy {
                message: response.message.unwrap_or_default(),
            }),
            "stopped" => Ok(AppStatus::Stopped),
            "failed" => Ok(AppStatus::Failed {
                message: response.message.unwrap_or_default(),
            }),
            _ => Ok(AppStatus::Unknown),
        }
    }

    fn deactivate(&self, ctx: &AppContext) -> AppsResult<()> {
        info!(
            app = ctx.app_name,
            "running generic orchestrator: deactivate"
        );
        Self::run_orchestrator_checked(ctx, "deactivate")
    }

    fn start(&self, ctx: &AppContext) -> AppsResult<()> {
        info!(app = ctx.app_name, "running generic orchestrator: start");
        Self::run_orchestrator_checked(ctx, "start")
    }

    fn stop(&self, ctx: &AppContext) -> AppsResult<()> {
        info!(app = ctx.app_name, "running generic orchestrator: stop");
        Self::run_orchestrator_checked(ctx, "stop")
    }
}
