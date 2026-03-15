use std::fs;
use std::path::{Path, PathBuf};

use reportify::ResultExt;
use tracing::{info, warn};

use super::orchestrator::{AppContext, AppStatus};
use super::{orchestrators, AppsResult};
use crate::config::apps::AppManifest;

/// Metadata for a single generation, persisted in `.rugix/generation.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppGeneration {
    /// Generation number.
    pub number: u64,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Timestamp of the last successful activation (RFC 3339), if any.
    #[serde(default)]
    pub last_activated: Option<String>,
}

/// A generation with its completeness status resolved from the filesystem.
pub struct ResolvedGeneration {
    /// The persisted generation metadata.
    pub meta: AppGeneration,
    /// Whether the generation is complete (has the `.rugix/complete` marker).
    pub complete: bool,
}

/// Persisted app state.
///
/// Stored at `<app_dir>/.rugix/state.json`. The `switching` state indicates an
/// in-progress transition.  If the system crashes while switching, recovery retries the
/// operation on the next boot.
///
/// When a switch fails (as opposed to being interrupted), the app automatically attempts
/// to roll back to the previous generation.  If rollback also fails, the app enters the
/// `error` state and requires manual intervention.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum AppState {
    /// No generation is active.
    Inactive,
    /// A transition is in progress.  Deactivates `from` (if set), then activates
    /// `to` (if set).  At least one of `from` and `to` must be `Some`.
    Switching {
        /// Generation being deactivated, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<u64>,
        /// Generation being activated, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        to: Option<u64>,
    },
    /// A generation is active and ready to run.
    Active {
        /// The active generation number.
        generation: u64,
    },
    /// A transition and automatic rollback both failed. Manual intervention required.
    Error {
        /// The generation that failed to activate.
        generation: u64,
        /// Human-readable error description.
        message: String,
    },
}

/// Manages app generations on the data partition.
pub struct AppManager {
    /// Root directory for all apps.
    apps_dir: PathBuf,
    /// The system's configured service manager (e.g., `"systemd"`, `"none"`).
    service_manager: String,
}

impl AppManager {
    pub fn new(apps_dir: PathBuf, service_manager: String) -> Self {
        Self {
            apps_dir,
            service_manager,
        }
    }

    fn app_dir(&self, app_name: &str) -> PathBuf {
        self.apps_dir.join(app_name)
    }

    fn generations_dir(&self, app_name: &str) -> PathBuf {
        self.app_dir(app_name).join("generations")
    }

    /// Path to a specific generation directory.
    pub fn generation_dir(&self, app_name: &str, number: u64) -> PathBuf {
        self.generations_dir(app_name).join(number.to_string())
    }

    fn data_dir(&self, app_name: &str) -> PathBuf {
        self.app_dir(app_name).join("data")
    }

    fn state_path(&self, app_name: &str) -> PathBuf {
        self.app_dir(app_name).join(".rugix/state.json")
    }

    fn write_state(&self, app_name: &str, state: &AppState) -> AppsResult<()> {
        let path = self.state_path(app_name);
        let content =
            serde_json::to_string_pretty(state).whatever("unable to serialize app state")?;
        rugix_common::fsutils::atomic_write(&path, content.as_bytes())
            .whatever("unable to write app state")?;
        Ok(())
    }

    /// Read the persisted app state, defaulting to `Inactive` if absent.
    pub fn read_state(&self, app_name: &str) -> AppState {
        let path = self.state_path(app_name);
        let Ok(content) = fs::read_to_string(&path) else {
            return AppState::Inactive;
        };
        serde_json::from_str(&content).unwrap_or(AppState::Inactive)
    }

    /// Check for and recover any interrupted transition for a single app.
    pub fn recover_app(&self, app_name: &str) -> AppsResult<()> {
        match self.read_state(app_name) {
            AppState::Switching { from, to } => {
                if let Some(to_gen) = to {
                    let gen_dir = self.generation_dir(app_name, to_gen);
                    if !gen_dir.exists() || !Self::is_complete(&gen_dir) {
                        warn!(
                            app = app_name,
                            generation = to_gen,
                            "interrupted switch but target generation is not complete, resetting to inactive"
                        );
                        self.write_state(app_name, &AppState::Inactive)?;
                        return Ok(());
                    }
                }
                info!(app = app_name, ?from, ?to, "recovering interrupted switch");
                self.do_switch(app_name, from, to, true)?;
            }
            // Nothing to recover.
            AppState::Inactive | AppState::Active { .. } | AppState::Error { .. } => {}
        }
        Ok(())
    }

    /// Check for and recover interrupted transitions across all apps.
    pub fn recover_all(&self) -> AppsResult<()> {
        let apps = self.list_apps()?;
        for app_name in &apps {
            if let Err(e) = self.recover_app(app_name) {
                warn!(app = %app_name, "recovery failed: {e:?}");
            }
        }
        Ok(())
    }

    /// Allocate the next generation number and create its directory.
    pub fn create_generation(&self, app_name: &str) -> AppsResult<(u64, PathBuf)> {
        let generations_dir = self.generations_dir(app_name);
        fs::create_dir_all(&generations_dir).whatever("unable to create generations directory")?;
        fs::create_dir_all(self.data_dir(app_name))
            .whatever("unable to create app data directory")?;

        let next = self.next_generation_number(app_name)?;
        let gen_dir = generations_dir.join(next.to_string());
        fs::create_dir_all(&gen_dir).whatever("unable to create generation directory")?;
        Ok((next, gen_dir))
    }

    fn next_generation_number(&self, app_name: &str) -> AppsResult<u64> {
        let generations_dir = self.generations_dir(app_name);
        let mut max = 0u64;
        if generations_dir.exists() {
            let entries =
                fs::read_dir(&generations_dir).whatever("unable to read generations directory")?;
            for entry in entries {
                let entry = entry.whatever("unable to read directory entry")?;
                if let Some(name) = entry.file_name().to_str() {
                    if let Ok(n) = name.parse::<u64>() {
                        max = max.max(n);
                    }
                }
            }
        }
        Ok(max + 1)
    }

    /// Write generation metadata.
    pub fn write_generation_metadata(
        &self,
        gen_dir: &Path,
        generation: &AppGeneration,
    ) -> AppsResult<()> {
        let metadata = serde_json::to_string_pretty(generation)
            .whatever("unable to serialize generation metadata")?;
        rugix_common::fsutils::atomic_write(
            &gen_dir.join(".rugix/generation.json"),
            metadata.as_bytes(),
        )
        .whatever("unable to write generation metadata")?;
        Ok(())
    }

    /// Mark a generation as complete (all payloads have been fully written).
    pub fn mark_complete(gen_dir: &Path) -> AppsResult<()> {
        let rugix_dir = gen_dir.join(".rugix");
        fs::create_dir_all(&rugix_dir).whatever("unable to create .rugix directory")?;
        fs::write(rugix_dir.join("complete"), "").whatever("unable to write complete marker")?;
        Ok(())
    }

    /// Check whether a generation is complete (fully installed).
    pub fn is_complete(gen_dir: &Path) -> bool {
        gen_dir.join(".rugix/complete").exists()
    }

    /// Update the generation metadata to record the current time as `last_activated`.
    fn mark_activated(gen_dir: &Path) -> AppsResult<()> {
        let meta_path = gen_dir.join(".rugix/generation.json");
        let content =
            fs::read_to_string(&meta_path).whatever("unable to read generation metadata")?;
        let mut gen: AppGeneration =
            serde_json::from_str(&content).whatever("unable to parse generation metadata")?;
        gen.last_activated = Some(jiff::Timestamp::now().to_string());
        let updated = serde_json::to_string_pretty(&gen)
            .whatever("unable to serialize generation metadata")?;
        rugix_common::fsutils::atomic_write(&meta_path, updated.as_bytes())
            .whatever("unable to write generation metadata")?;
        Ok(())
    }

    /// Activate a generation: set up resources, start the workload, and
    /// register auto-start behaviour.
    ///
    /// If another generation is currently active it is deactivated first.
    /// If activation fails, the previous generation is automatically rolled back.
    /// If rollback also fails, the app enters the `error` state.
    pub fn activate_generation(&self, app_name: &str, gen_number: u64) -> AppsResult<()> {
        let gen_dir = self.generation_dir(app_name, gen_number);
        if !Self::is_complete(&gen_dir) {
            reportify::bail!("generation is not complete (installation may have been interrupted)");
        }

        let from = self.current_generation(app_name);
        self.write_state(
            app_name,
            &AppState::Switching {
                from,
                to: Some(gen_number),
            },
        )?;

        self.do_switch(app_name, from, Some(gen_number), false)
    }

    /// Deactivate the current generation.
    pub fn deactivate(&self, app_name: &str) -> AppsResult<()> {
        let Some(current) = self.current_generation(app_name) else {
            reportify::bail!("app {app_name} has no active generation");
        };

        self.write_state(
            app_name,
            &AppState::Switching {
                from: Some(current),
                to: None,
            },
        )?;

        self.do_switch(app_name, Some(current), None, false)
    }

    /// Execute a switch: deactivate `from` (if set), then activate `to` (if set).
    ///
    /// On activation failure, attempts to roll back to the `from` generation.
    /// If rollback also fails, transitions to the `Error` state.
    fn do_switch(
        &self,
        app_name: &str,
        from: Option<u64>,
        to: Option<u64>,
        recovery: bool,
    ) -> AppsResult<()> {
        // Phase 1: deactivate the old generation.
        if let Some(from_gen) = from {
            self.run_deactivate(app_name, from_gen, recovery)?;
        }

        // Phase 2: activate the new generation.
        let Some(to_gen) = to else {
            // Pure deactivation, already done.
            self.write_state(app_name, &AppState::Inactive)?;
            info!(app = app_name, recovery, "generation deactivated");
            return Ok(());
        };

        let gen_dir = self.generation_dir(app_name, to_gen);
        if let Err(err) = self.run_activate(app_name, &gen_dir, recovery) {
            warn!(
                app = app_name,
                generation = to_gen,
                "activation failed: {err:?}"
            );
            // Attempt rollback to the previous generation.
            if let Some(prev) = from {
                let prev_dir = self.generation_dir(app_name, prev);
                if prev_dir.exists() && Self::is_complete(&prev_dir) {
                    info!(
                        app = app_name,
                        from = to_gen,
                        to = prev,
                        "rolling back to previous generation"
                    );
                    self.write_state(
                        app_name,
                        &AppState::Switching {
                            from: None,
                            to: Some(prev),
                        },
                    )?;
                    if let Err(rollback_err) = self.run_activate(app_name, &prev_dir, true) {
                        warn!(
                            app = app_name,
                            generation = prev,
                            "rollback also failed: {rollback_err:?}"
                        );
                        self.write_state(
                            app_name,
                            &AppState::Error {
                                generation: to_gen,
                                message: format!(
                                    "activation failed and rollback to generation {prev} also failed"
                                ),
                            },
                        )?;
                        return Err(err);
                    }
                    // Rollback succeeded.
                    return Ok(());
                }
            }
            // No previous generation to roll back to.
            self.write_state(
                app_name,
                &AppState::Error {
                    generation: to_gen,
                    message: format!("activation failed: {err:?}"),
                },
            )?;
            return Err(err);
        }

        Ok(())
    }

    /// Run the orchestrator's activate hook.
    fn run_activate(&self, app_name: &str, gen_dir: &Path, recovery: bool) -> AppsResult<()> {
        let manifest = load_manifest(gen_dir)?;
        let orchestrator = orchestrators::get(manifest.orchestrator.as_str())?;
        let app_dir = self.app_dir(app_name);
        let data_dir = self.data_dir(app_name);
        let ctx = AppContext {
            app_name,
            app_dir: &app_dir,
            generation_dir: gen_dir,
            data_dir: &data_dir,
            recovery,
            service_manager: &self.service_manager,
        };

        orchestrator
            .activate(&ctx)
            .whatever("orchestrator activation failed")?;

        Self::mark_activated(gen_dir)?;

        let gen_number = gen_dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        self.write_state(
            app_name,
            &AppState::Active {
                generation: gen_number,
            },
        )?;
        info!(
            app = app_name,
            generation_dir = ?gen_dir,
            recovery,
            "generation activated"
        );
        Ok(())
    }

    /// Run the orchestrator's deactivate hook for a specific generation.
    fn run_deactivate(&self, app_name: &str, gen_number: u64, recovery: bool) -> AppsResult<()> {
        let gen_dir = self.generation_dir(app_name, gen_number);
        if !gen_dir.exists() {
            return Ok(());
        }
        let manifest = load_manifest(&gen_dir)?;
        let orchestrator = orchestrators::get(manifest.orchestrator.as_str())?;
        let app_dir = self.app_dir(app_name);
        let data_dir = self.data_dir(app_name);
        let ctx = AppContext {
            app_name,
            app_dir: &app_dir,
            generation_dir: &gen_dir,
            data_dir: &data_dir,
            recovery,
            service_manager: &self.service_manager,
        };

        orchestrator
            .deactivate(&ctx)
            .whatever("orchestrator deactivation failed")?;
        Ok(())
    }

    /// Get status of the currently active generation.
    pub fn app_status(&self, app_name: &str) -> AppsResult<AppStatus> {
        let Some(gen_dir) = self.resolve_current(app_name) else {
            return Ok(AppStatus::Stopped);
        };
        let manifest = load_manifest(&gen_dir)?;
        let orchestrator = orchestrators::get(manifest.orchestrator.as_str())?;
        let app_dir = self.app_dir(app_name);
        let data_dir = self.data_dir(app_name);
        let ctx = AppContext {
            app_name,
            app_dir: &app_dir,
            generation_dir: &gen_dir,
            data_dir: &data_dir,
            recovery: false,
            service_manager: &self.service_manager,
        };
        orchestrator
            .status(&ctx)
            .whatever("failed to get app status")
    }

    /// List all installed apps.
    pub fn list_apps(&self) -> AppsResult<Vec<String>> {
        let mut apps = Vec::new();
        if !self.apps_dir.exists() {
            return Ok(apps);
        }
        let entries = fs::read_dir(&self.apps_dir).whatever("unable to read apps directory")?;
        for entry in entries {
            let entry = entry.whatever("unable to read directory entry")?;
            if entry
                .file_type()
                .whatever("unable to get file type")?
                .is_dir()
            {
                if let Some(name) = entry.file_name().to_str() {
                    apps.push(name.to_owned());
                }
            }
        }
        apps.sort();
        Ok(apps)
    }

    /// List generations for a given app.
    pub fn list_generations(&self, app_name: &str) -> AppsResult<Vec<ResolvedGeneration>> {
        let generations_dir = self.generations_dir(app_name);
        let mut generations = Vec::new();
        if !generations_dir.exists() {
            return Ok(generations);
        }
        let entries =
            fs::read_dir(&generations_dir).whatever("unable to read generations directory")?;
        for entry in entries {
            let entry = entry.whatever("unable to read directory entry")?;
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(number) = name.parse::<u64>() {
                    let gen_dir = entry.path();
                    let complete = Self::is_complete(&gen_dir);
                    let meta_path = gen_dir.join(".rugix").join("generation.json");
                    let meta = if let Ok(content) = fs::read_to_string(&meta_path) {
                        serde_json::from_str::<AppGeneration>(&content).ok()
                    } else {
                        None
                    };
                    let meta = meta.unwrap_or(AppGeneration {
                        number,
                        created_at: String::new(),
                        last_activated: None,
                    });
                    generations.push(ResolvedGeneration { meta, complete });
                }
            }
        }
        generations.sort_by_key(|g| g.meta.number);
        Ok(generations)
    }

    /// Get the currently active generation number, if any.
    pub fn current_generation(&self, app_name: &str) -> Option<u64> {
        match self.read_state(app_name) {
            AppState::Active { generation } => Some(generation),
            _ => None,
        }
    }

    /// Find the most recently activated generation (by `lastActivated` timestamp).
    pub fn last_activated_generation(&self, app_name: &str) -> AppsResult<Option<u64>> {
        let generations = self.list_generations(app_name)?;
        let best = generations
            .iter()
            .filter_map(|g| {
                g.meta
                    .last_activated
                    .as_deref()
                    .map(|ts| (g.meta.number, ts))
            })
            .max_by_key(|(_num, ts)| ts.to_owned());
        Ok(best.map(|(num, _)| num))
    }

    /// Rollback: deactivate the current generation and activate the most recent
    /// previous generation that was successfully activated before.
    pub fn rollback(&self, app_name: &str) -> AppsResult<()> {
        let Some(current) = self.current_generation(app_name) else {
            reportify::bail!("no current generation to rollback from");
        };
        let generations = self.list_generations(app_name)?;
        let Some(previous) = generations
            .iter()
            .rev()
            .find(|g| g.meta.number < current && g.meta.last_activated.is_some())
        else {
            reportify::bail!("no previous activated generation to rollback to");
        };
        info!(
            app = app_name,
            from = current,
            to = previous.meta.number,
            "rolling back"
        );
        self.activate_generation(app_name, previous.meta.number)
    }

    /// Garbage collect old generations.
    ///
    /// Generations that were never activated are always removed (they are not
    /// valid rollback targets).  Among previously-activated generations, at
    /// most `keep` of the most recent ones are retained.  The currently active
    /// generation is never removed.
    pub fn gc(&self, app_name: &str, keep: usize) -> AppsResult<Vec<u64>> {
        let current = self.current_generation(app_name);
        let mut generations = self.list_generations(app_name)?;
        generations.sort_by_key(|g| g.meta.number);
        let mut removed = Vec::new();

        // Remove all never-activated generations (except the current one).
        for gen in &generations {
            if Some(gen.meta.number) == current {
                continue;
            }
            if gen.meta.last_activated.is_none() {
                let gen_dir = self
                    .generations_dir(app_name)
                    .join(gen.meta.number.to_string());
                if let Err(e) = fs::remove_dir_all(&gen_dir) {
                    info!(
                        generation = gen.meta.number,
                        "failed to remove generation: {e}"
                    );
                } else {
                    removed.push(gen.meta.number);
                }
            }
        }

        // Among previously-activated generations, keep the most recent `keep`.
        let activated: Vec<_> = generations
            .iter()
            .filter(|g| g.meta.last_activated.is_some() && Some(g.meta.number) != current)
            .collect();
        if activated.len() > keep {
            let to_remove = activated.len() - keep;
            for gen in activated.iter().take(to_remove) {
                let gen_dir = self
                    .generations_dir(app_name)
                    .join(gen.meta.number.to_string());
                if let Err(e) = fs::remove_dir_all(&gen_dir) {
                    info!(
                        generation = gen.meta.number,
                        "failed to remove generation: {e}"
                    );
                } else {
                    removed.push(gen.meta.number);
                }
            }
        }

        removed.sort();
        Ok(removed)
    }

    /// Remove an app entirely: deactivate, delete all generations, data, and
    /// system files.
    pub fn remove_app(&self, app_name: &str) -> AppsResult<()> {
        // Deactivate if active (stops workload + cleans up orchestrator resources).
        if self.current_generation(app_name).is_some() {
            self.deactivate(app_name)?;
        }
        let app_dir = self.app_dir(app_name);
        if app_dir.exists() {
            fs::remove_dir_all(&app_dir).whatever("unable to remove app directory")?;
        }
        info!(app = app_name, "app removed");
        Ok(())
    }

    /// Resolve the active generation directory from the persisted state.
    fn resolve_current(&self, app_name: &str) -> Option<PathBuf> {
        let gen = self.current_generation(app_name)?;
        let dir = self.generation_dir(app_name, gen);
        dir.exists().then_some(dir)
    }
}

fn load_manifest(gen_dir: &Path) -> AppsResult<AppManifest> {
    let manifest_path = gen_dir.join("app.toml");
    let content = fs::read_to_string(&manifest_path).whatever("unable to read app.toml")?;
    toml::from_str(&content).whatever("unable to parse app.toml")
}
