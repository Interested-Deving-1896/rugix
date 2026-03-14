use std::path::Path;

use super::AppsResult;

/// Status of an app's workload.
#[derive(Debug, Clone)]
pub enum AppStatus {
    Running,
    Stopped,
    Failed { message: String },
    Unknown,
}

impl std::fmt::Display for AppStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppStatus::Running => write!(f, "running"),
            AppStatus::Stopped => write!(f, "stopped"),
            AppStatus::Failed { message } => write!(f, "failed: {message}"),
            AppStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// Context passed to orchestrator methods.
pub struct AppContext<'cx> {
    /// App name.
    pub app_name: &'cx str,
    /// Path to the app directory.
    pub app_dir: &'cx Path,
    /// Path to the generation directory.
    pub generation_dir: &'cx Path,
    /// Path to persistent app data (survives across generations).
    pub data_dir: &'cx Path,
    /// Whether this invocation is recovering from an interrupted transition.
    pub recovery: bool,
    /// The system's configured service manager (e.g., `"systemd"`, `"none"`).
    pub service_manager: &'cx str,
}

/// Trait for app lifecycle orchestrators.
///
/// Activation must set up all resources, start the workload, and register auto-start
/// behavior (e.g., `systemctl enable`).  Deactivation must stop the workload, disable
/// auto-start, and tear down resources.
pub trait Orchestrator: Send + Sync {
    /// Unique identifier (e.g., "docker-compose").
    fn name(&self) -> &str;

    /// Activate a generation: set up resources, start the workload, and register
    /// auto-start behavior.
    fn activate(&self, ctx: &AppContext) -> AppsResult<()>;

    /// Query live status.
    fn status(&self, ctx: &AppContext) -> AppsResult<AppStatus>;

    /// Deactivate a generation: stop the workload, disable auto-start, and tear down
    /// resources.
    fn deactivate(&self, ctx: &AppContext) -> AppsResult<()>;
}
