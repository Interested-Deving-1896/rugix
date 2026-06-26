use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Json;
use serde_json::json;
use serde_json::Value;

use crate::generated::api;

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
    details: Option<Value>,
}

impl ApiError {
    pub(crate) fn new(
        status: StatusCode,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub(crate) fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub(crate) fn conflict(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub(crate) fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub(crate) fn command_spawn(command: &str, err: std::io::Error) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "command-spawn-failed",
            format!("unable to spawn {command}: {err}"),
        )
    }

    pub(crate) fn command_failed(
        command: &str,
        args: &[&str],
        output: &std::process::Output,
    ) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "command-failed".to_owned(),
            message: format!("{command} exited with {}", output.status),
            details: Some(json!({
                "args": args,
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            })),
        }
    }

    pub(crate) fn invalid_ctrl_output(err: serde_json::Error) -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "invalid-ctrl-output",
            format!("rugix-ctrl returned invalid JSON: {err}"),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let Self {
            status,
            code,
            message,
            details,
        } = self;
        if status.is_server_error() {
            tracing::error!(%status, %code, %message, details = ?details, "API request failed");
        } else {
            tracing::warn!(%status, %code, %message, details = ?details, "API request failed");
        }
        let body =
            api::ApiErrorResponse::new(api::ApiError::new(code, message).with_details(details));
        (status, Json(body)).into_response()
    }
}
