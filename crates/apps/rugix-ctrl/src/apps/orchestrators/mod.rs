mod binary;
mod docker_compose;
mod generic;

use super::orchestrator::Orchestrator;
use super::AppsResult;

/// Look up an orchestrator by name.
pub fn get(name: &str) -> AppsResult<Box<dyn Orchestrator>> {
    match name {
        "docker-compose" => Ok(Box::new(docker_compose::DockerCompose)),
        "binary" => Ok(Box::new(binary::Binary)),
        "generic" => Ok(Box::new(generic::Generic)),
        other => {
            reportify::bail!("unsupported orchestrator: {other}");
        }
    }
}
