use std::path::{Path, PathBuf};
use std::{fs, io};

use reportify::{bail, whatever, ErrorExt, ResultExt};
use rugix_component_set::{
    Capability, CapabilitySelector, Component, ComponentId, ComponentSet, Problem,
};

use crate::apps::manager::AppManager;
use crate::config::output::{
    CapabilityOutput, CapabilitySelectorOutput, ComponentConflictProblemOutput, ComponentOutput,
    ComponentProblemOutput, ComponentRefOutput, ComponentRootOutput, ComponentSourceKindOutput,
    ComponentSourceOutput, ComponentsCheckOutput, ComponentsOutput,
    DuplicateComponentProblemOutput, LoadedComponentOutput, UnsatisfiedRequirementProblemOutput,
};
use crate::system::SystemResult;

const SYSTEM_COMPONENTS_DIR: &str = "/usr/lib/rugix/components";
const LOCAL_COMPONENTS_DIR: &str = "/etc/rugix/components";
const RUNTIME_COMPONENTS_DIR: &str = "/run/rugix/components";

/// Installed components loaded from all active component roots.
#[derive(Debug, Clone)]
pub struct InstalledComponents {
    roots: Vec<ComponentLocation>,
    components: Vec<LoadedComponent>,
}

impl InstalledComponents {
    /// Load installed components from the standard Rugix component roots.
    pub fn load() -> SystemResult<Self> {
        let mut components = Self {
            roots: Vec::new(),
            components: Vec::new(),
        };
        components.load_root(ComponentLocation::new(
            ComponentSourceKindOutput::System,
            SYSTEM_COMPONENTS_DIR,
        ))?;
        components.load_root(ComponentLocation::new(
            ComponentSourceKindOutput::Local,
            LOCAL_COMPONENTS_DIR,
        ))?;
        components.load_root(ComponentLocation::new(
            ComponentSourceKindOutput::Runtime,
            RUNTIME_COMPONENTS_DIR,
        ))?;
        components.load_active_app_roots()?;
        Ok(components)
    }

    /// Build inventory output for all loaded components.
    pub fn output(&self) -> ComponentsOutput {
        ComponentsOutput::new(self.root_outputs(), self.component_outputs())
    }

    /// Build inventory output for components with the given component ID.
    pub fn output_for_component(&self, component_id: &str) -> SystemResult<ComponentsOutput> {
        let components = self
            .components
            .iter()
            .filter(|component| component.component.id().as_str() == component_id)
            .map(LoadedComponent::output)
            .collect::<Vec<_>>();
        if components.is_empty() {
            bail!("component {component_id:?} not found");
        }
        Ok(ComponentsOutput::new(self.root_outputs(), components))
    }

    /// Check the loaded component set for internal consistency.
    pub fn check_output(&self) -> ComponentsCheckOutput {
        let component_set = ComponentSet::new(
            self.components
                .iter()
                .map(|component| component.component.clone())
                .collect(),
        );
        let report = component_set.check();
        let consistent = report.is_consistent();
        let problems = report
            .problems()
            .iter()
            .map(|problem| self.problem_output(problem))
            .collect();
        ComponentsCheckOutput::new(
            self.root_outputs(),
            self.component_outputs(),
            consistent,
            problems,
        )
    }

    fn root_outputs(&self) -> Vec<ComponentRootOutput> {
        self.roots
            .iter()
            .map(ComponentLocation::root_output)
            .collect()
    }

    fn component_outputs(&self) -> Vec<LoadedComponentOutput> {
        self.components
            .iter()
            .map(LoadedComponent::output)
            .collect()
    }

    fn load_root(&mut self, root: ComponentLocation) -> SystemResult<()> {
        for path in find_component_files(&root.path)? {
            let component = read_component_file(&path)?;
            self.components.push(LoadedComponent {
                source: root.file_location(path),
                component,
            });
        }

        self.roots.push(root);
        Ok(())
    }

    fn load_active_app_roots(&mut self) -> SystemResult<()> {
        let apps_config =
            crate::apps::config::load_apps_config().whatever("unable to load apps config")?;
        let apps_dir = crate::apps::config::apps_dir().to_owned();
        let manager = AppManager::new(apps_dir, apps_config);
        let apps = manager.list_apps().whatever("unable to list apps")?;
        for app in apps {
            let Some(generation) = manager
                .current_generation(&app)
                .whatever("unable to read app state")
                .with_info(|_| format!("app: {app}"))?
            else {
                continue;
            };
            let root_path = manager
                .generation_dir(&app, generation)
                .join(".rugix/components");
            self.load_root(ComponentLocation::app(app, generation, root_path))?;
        }
        Ok(())
    }

    fn problem_output(&self, problem: &Problem) -> ComponentProblemOutput {
        match problem {
            Problem::DuplicateComponent { id } => {
                ComponentProblemOutput::DuplicateComponent(DuplicateComponentProblemOutput::new(
                    id.to_string(),
                    self.sources_for_component(id),
                ))
            }
            Problem::UnsatisfiedRequirement {
                component,
                selector,
            } => ComponentProblemOutput::UnsatisfiedRequirement(
                UnsatisfiedRequirementProblemOutput::new(
                    self.component_ref_output(component),
                    selector_output(selector),
                ),
            ),
            Problem::Conflict {
                component,
                selector,
                provider,
                capability,
            } => ComponentProblemOutput::Conflict(ComponentConflictProblemOutput::new(
                self.component_ref_output(component),
                selector_output(selector),
                self.component_ref_output(provider),
                capability_output(capability),
            )),
        }
    }

    fn component_ref_output(&self, component_id: &ComponentId) -> ComponentRefOutput {
        ComponentRefOutput::new(component_id.to_string())
            .with_source(self.source_for_component(component_id))
    }

    fn source_for_component(&self, component_id: &ComponentId) -> Option<ComponentSourceOutput> {
        self.components
            .iter()
            .find(|component| component.component.id() == component_id)
            .map(|component| component.source.source_output())
    }

    fn sources_for_component(&self, component_id: &ComponentId) -> Vec<ComponentSourceOutput> {
        self.components
            .iter()
            .filter(|component| component.component.id() == component_id)
            .map(|component| component.source.source_output())
            .collect()
    }
}

#[derive(Debug, Clone)]
struct LoadedComponent {
    source: ComponentLocation,
    component: Component,
}

impl LoadedComponent {
    fn output(&self) -> LoadedComponentOutput {
        LoadedComponentOutput::new(
            self.source.source_output(),
            component_output(&self.component),
        )
    }
}

#[derive(Debug, Clone)]
struct ComponentLocation {
    kind: ComponentSourceKindOutput,
    path: PathBuf,
    app: Option<String>,
    generation: Option<u64>,
}

impl ComponentLocation {
    fn new(kind: ComponentSourceKindOutput, path: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            path: path.into(),
            app: None,
            generation: None,
        }
    }

    fn app(app: String, generation: u64, path: impl Into<PathBuf>) -> Self {
        Self {
            kind: ComponentSourceKindOutput::App,
            path: path.into(),
            app: Some(app),
            generation: Some(generation),
        }
    }

    fn file_location(&self, path: PathBuf) -> Self {
        Self {
            kind: self.kind.clone(),
            path,
            app: self.app.clone(),
            generation: self.generation,
        }
    }

    fn root_output(&self) -> ComponentRootOutput {
        ComponentRootOutput::new(self.kind.clone(), self.path.to_string_lossy().into_owned())
            .with_app(self.app.clone())
            .with_generation(self.generation)
    }

    fn source_output(&self) -> ComponentSourceOutput {
        ComponentSourceOutput::new(self.kind.clone(), self.path.to_string_lossy().into_owned())
            .with_app(self.app.clone())
            .with_generation(self.generation)
    }
}

fn component_output(component: &Component) -> ComponentOutput {
    ComponentOutput::new(
        component.id().to_string(),
        component.provides().iter().map(capability_output).collect(),
        component.requires().iter().map(selector_output).collect(),
        component.conflicts().iter().map(selector_output).collect(),
    )
    .with_version(component.version().map(ToString::to_string))
}

fn capability_output(capability: &Capability) -> CapabilityOutput {
    CapabilityOutput::new(capability.id().to_string())
        .with_version(capability.version().map(ToString::to_string))
        .with_value(capability.value_str().map(str::to_owned))
}

fn selector_output(selector: &CapabilitySelector) -> CapabilitySelectorOutput {
    CapabilitySelectorOutput::new(selector.id().to_string())
        .with_version(selector.version_req().map(ToString::to_string))
        .with_value(selector.value_str().map(str::to_owned))
}

fn find_component_files(root: &Path) -> SystemResult<Vec<PathBuf>> {
    let mut component_files = Vec::new();
    collect_component_files(root, &mut component_files)?;
    component_files.sort();
    Ok(component_files)
}

fn collect_component_files(root: &Path, component_files: &mut Vec<PathBuf>) -> SystemResult<()> {
    let metadata = match fs::metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error
                .whatever("unable to inspect component root")
                .with_info(format!("path: {root:?}")));
        }
    };
    if !metadata.is_dir() {
        return Err(
            whatever!("component root is not a directory").with_info(format!("path: {root:?}"))
        );
    }

    let entries = fs::read_dir(root)
        .whatever("unable to read component directory")
        .with_info(|_| format!("path: {root:?}"))?;
    for entry in entries {
        let entry = entry
            .whatever("unable to read component directory entry")
            .with_info(|_| format!("path: {root:?}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .whatever("unable to inspect component directory entry")
            .with_info(|_| format!("path: {path:?}"))?;
        if file_type.is_dir() {
            collect_component_files(&path, component_files)?;
        } else if is_component_file(&path) && is_regular_file(&path)? {
            component_files.push(path);
        }
    }

    Ok(())
}

fn read_component_file(path: &Path) -> SystemResult<Component> {
    let content = fs::read_to_string(path)
        .whatever("unable to read component file")
        .with_info(|_| format!("path: {path:?}"))?;
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("json") {
        serde_json::from_str(&content)
            .whatever("unable to parse JSON component file")
            .with_info(|_| format!("path: {path:?}"))
    } else {
        toml::from_str(&content)
            .whatever("unable to parse TOML component file")
            .with_info(|_| format!("path: {path:?}"))
    }
}

fn is_component_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("toml") || extension.eq_ignore_ascii_case("json")
        })
}

fn is_regular_file(path: &Path) -> SystemResult<bool> {
    fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .whatever("unable to inspect component file")
        .with_info(|_| format!("path: {path:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_components_recursively_in_path_order() {
        let tempdir = tempfile::tempdir().unwrap();
        let root = tempdir.path().join("components");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(
            root.join("z.toml"),
            r#"
id = "component.z"
version = "1.0.0"
"#,
        )
        .unwrap();
        fs::write(
            root.join("nested/a.toml"),
            r#"
id = "component.a"
"#,
        )
        .unwrap();

        let mut components = InstalledComponents {
            roots: Vec::new(),
            components: Vec::new(),
        };
        components
            .load_root(ComponentLocation::new(
                ComponentSourceKindOutput::Local,
                root,
            ))
            .unwrap();

        let ids = components
            .components
            .iter()
            .map(|component| component.component.id().as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["component.a", "component.z"]);
    }

    #[test]
    fn reports_duplicate_component_sources() {
        let source_a = ComponentLocation {
            kind: ComponentSourceKindOutput::Local,
            path: PathBuf::from("/etc/rugix/components/a.toml"),
            app: None,
            generation: None,
        };
        let source_b = ComponentLocation {
            kind: ComponentSourceKindOutput::Runtime,
            path: PathBuf::from("/run/rugix/components/b.toml"),
            app: None,
            generation: None,
        };
        let components = InstalledComponents {
            roots: Vec::new(),
            components: vec![
                LoadedComponent {
                    source: source_a,
                    component: Component::new("component.duplicate"),
                },
                LoadedComponent {
                    source: source_b,
                    component: Component::new("component.duplicate"),
                },
            ],
        };

        let output = components.check_output();
        assert!(!output.consistent);
        assert_eq!(output.problems.len(), 1);
        let ComponentProblemOutput::DuplicateComponent(problem) = &output.problems[0] else {
            panic!("expected duplicate component problem");
        };
        assert_eq!(problem.id, "component.duplicate");
        assert_eq!(problem.sources.len(), 2);
    }

    #[test]
    fn reports_conflict_participants_as_component_refs() {
        let provider_source = ComponentLocation {
            kind: ComponentSourceKindOutput::Local,
            path: PathBuf::from("/etc/rugix/components/provider.toml"),
            app: None,
            generation: None,
        };
        let consumer_source = ComponentLocation {
            kind: ComponentSourceKindOutput::Runtime,
            path: PathBuf::from("/run/rugix/components/consumer.toml"),
            app: None,
            generation: None,
        };
        let components = InstalledComponents {
            roots: Vec::new(),
            components: vec![
                LoadedComponent {
                    source: provider_source,
                    component: Component::new("component.provider")
                        .with_provided_capability(Capability::new("service.modbus")),
                },
                LoadedComponent {
                    source: consumer_source,
                    component: Component::new("component.consumer")
                        .with_conflict(CapabilitySelector::new("service.modbus")),
                },
            ],
        };

        let output = components.check_output();
        assert!(!output.consistent);
        assert_eq!(output.problems.len(), 1);
        let ComponentProblemOutput::Conflict(problem) = &output.problems[0] else {
            panic!("expected conflict problem");
        };
        assert_eq!(problem.component.id, "component.consumer");
        assert_eq!(
            problem
                .component
                .source
                .as_ref()
                .map(|source| source.path.as_str()),
            Some("/run/rugix/components/consumer.toml")
        );
        assert_eq!(problem.provider.id, "component.provider");
        assert_eq!(
            problem
                .provider
                .source
                .as_ref()
                .map(|source| source.path.as_str()),
            Some("/etc/rugix/components/provider.toml")
        );
    }
}
