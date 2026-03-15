pub mod config;
pub mod generator;
pub mod manager;
pub mod orchestrator;
pub mod orchestrators;

use reportify::Report;

reportify::new_whatever_type! {
    AppsError
}

pub type AppsResult<T> = Result<T, Report<AppsError>>;
