use std::net::SocketAddr;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::{Router, Server};
use clap::Parser;
use include_dir::{include_dir, Dir};

mod assets;
mod ctrl;
mod error;
mod handlers;
mod jobs;

sidex::include_bundle!(pub rugix_admin as generated);

use error::ApiError;
use jobs::JobManager;

static FRONTEND: Dir<'_> = include_dir!("$OUT_DIR/frontend-dist");

type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone, Parser)]
pub struct Args {
    /// The address to bind to.
    #[clap(long, default_value = "0.0.0.0:8088")]
    pub address: SocketAddr,
}

#[derive(Debug, Clone)]
pub(crate) struct ServerState {
    jobs: JobManager,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let state = ServerState {
        jobs: JobManager::default(),
    };

    let app = Router::new()
        .route("/api/health", get(handlers::health))
        .route("/api/system/info", get(handlers::system_info))
        .route("/api/components", get(handlers::components))
        .route(
            "/api/system/update/:job_id",
            post(handlers::upload_system_update),
        )
        .route("/api/system/actions/:action", post(handlers::system_action))
        .route("/api/apps", get(handlers::list_apps))
        .route(
            "/api/apps/install/:job_id",
            post(handlers::upload_app_bundle),
        )
        .route("/api/apps/:app", get(handlers::app_info))
        .route("/api/apps/:app/actions/:action", post(handlers::app_action))
        .route("/api/jobs", get(handlers::list_jobs))
        .route("/api/jobs/:job_id", get(handlers::get_job))
        .route("/api/jobs/:job_id/events", get(handlers::job_events))
        .fallback(assets::static_asset)
        .layer(DefaultBodyLimit::disable())
        .with_state(state);

    Server::bind(&args.address)
        .serve(app.into_make_service())
        .await
        .expect("failed to serve Rugix Admin");
}
