use std::process::Stdio;

use axum::extract::Multipart;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::error::ApiError;
use crate::generated::jobs;
use crate::jobs::JobManager;
use crate::ApiResult;

#[derive(Debug, Clone)]
pub(crate) struct CommandSpec {
    pub(crate) title: String,
    pub(crate) kind: String,
    pub(crate) target: Option<String>,
    pub(crate) args: Vec<String>,
}

impl CommandSpec {
    pub(crate) fn new(title: &str, kind: &str, target: Option<String>, args: Vec<String>) -> Self {
        Self {
            title: title.to_owned(),
            kind: kind.to_owned(),
            target,
            args,
        }
    }
}

pub(crate) async fn run_json_command(args: &[&str]) -> ApiResult<Value> {
    let output = Command::new("rugix-ctrl")
        .args(args)
        .output()
        .await
        .map_err(|err| ApiError::command_spawn("rugix-ctrl", err))?;

    if !output.status.success() {
        return Err(ApiError::command_failed("rugix-ctrl", args, &output));
    }

    serde_json::from_slice(&output.stdout).map_err(ApiError::invalid_ctrl_output)
}

pub(crate) async fn run_components_check_command() -> ApiResult<Value> {
    let args = ["components", "check"];
    let output = Command::new("rugix-ctrl")
        .args(args)
        .output()
        .await
        .map_err(|err| ApiError::command_spawn("rugix-ctrl", err))?;

    match output.status.code() {
        Some(0 | 1) => {
            serde_json::from_slice(&output.stdout).map_err(ApiError::invalid_ctrl_output)
        }
        _ => Err(ApiError::command_failed("rugix-ctrl", &args, &output)),
    }
}

pub(crate) fn spawn_command_job(jobs: JobManager, job_id: String, args: Vec<String>) {
    tokio::spawn(async move {
        jobs.set_status(&job_id, jobs::JobStatus::Running).await;
        let mut child = match Command::new("rugix-ctrl")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                jobs.fail(&job_id, format!("unable to spawn rugix-ctrl: {err}"), None)
                    .await;
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_task = stdout.map(|stdout| {
            tokio::spawn(read_output_lines(
                jobs.clone(),
                job_id.clone(),
                "stdout",
                stdout,
            ))
        });
        let stderr_task = stderr.map(|stderr| {
            tokio::spawn(read_output_lines(
                jobs.clone(),
                job_id.clone(),
                "stderr",
                stderr,
            ))
        });

        match child.wait().await {
            Ok(status) if status.success() => {
                jobs.set_status(&job_id, jobs::JobStatus::Succeeded).await
            }
            Ok(status) => {
                jobs.fail(
                    &job_id,
                    format!("rugix-ctrl exited with {status}"),
                    status.code(),
                )
                .await;
            }
            Err(err) => {
                jobs.fail(
                    &job_id,
                    format!("unable to wait for rugix-ctrl: {err}"),
                    None,
                )
                .await;
            }
        }

        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        if let Some(task) = stderr_task {
            let _ = task.await;
        }
    });
}

pub(crate) async fn stream_upload_job(
    jobs: JobManager,
    job_id: String,
    args: Vec<String>,
    mut multipart: Multipart,
    file_field: &'static str,
) {
    jobs.set_status(&job_id, jobs::JobStatus::Running).await;
    let mut child = match Command::new("rugix-ctrl")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            jobs.fail(&job_id, format!("unable to spawn rugix-ctrl: {err}"), None)
                .await;
            return;
        }
    };

    let stdout_task = child.stdout.take().map(|stdout| {
        tokio::spawn(read_output_lines(
            jobs.clone(),
            job_id.clone(),
            "stdout",
            stdout,
        ))
    });
    let stderr_task = child.stderr.take().map(|stderr| {
        tokio::spawn(read_output_lines(
            jobs.clone(),
            job_id.clone(),
            "stderr",
            stderr,
        ))
    });

    let Some(mut stdin) = child.stdin.take() else {
        jobs.fail(&job_id, "rugix-ctrl stdin is unavailable".to_owned(), None)
            .await;
        return;
    };

    let mut found_file = false;
    let mut bytes = 0u64;
    loop {
        match multipart.next_field().await {
            Ok(Some(mut field)) => {
                if field.name() != Some(file_field) {
                    continue;
                }
                found_file = true;
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            if let Err(err) = stdin.write_all(&chunk).await {
                                jobs.fail(
                                    &job_id,
                                    format!("unable to stream upload to rugix-ctrl: {err}"),
                                    None,
                                )
                                .await;
                                drop(stdin);
                                wait_after_upload(jobs, job_id, child, stdout_task, stderr_task)
                                    .await;
                                return;
                            }
                            bytes += chunk.len() as u64;
                            jobs.emit_upload_progress(&job_id, bytes).await;
                        }
                        Ok(None) => break,
                        Err(err) => {
                            jobs.fail(
                                &job_id,
                                format!("unable to read upload stream: {err}"),
                                None,
                            )
                            .await;
                            drop(stdin);
                            wait_after_upload(jobs, job_id, child, stdout_task, stderr_task).await;
                            return;
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(err) => {
                jobs.fail(&job_id, format!("invalid multipart upload: {err}"), None)
                    .await;
                drop(stdin);
                wait_after_upload(jobs, job_id, child, stdout_task, stderr_task).await;
                return;
            }
        }
    }

    if !found_file {
        jobs.fail(&job_id, format!("missing `{file_field}` file field"), None)
            .await;
    }

    drop(stdin);
    wait_after_upload(jobs, job_id, child, stdout_task, stderr_task).await;
}

async fn wait_after_upload(
    jobs: JobManager,
    job_id: String,
    mut child: tokio::process::Child,
    stdout_task: Option<tokio::task::JoinHandle<()>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
) {
    match child.wait().await {
        Ok(status) if status.success() => {
            jobs.set_status(&job_id, jobs::JobStatus::Succeeded).await
        }
        Ok(status) => {
            jobs.fail(
                &job_id,
                format!("rugix-ctrl exited with {status}"),
                status.code(),
            )
            .await;
        }
        Err(err) => {
            jobs.fail(
                &job_id,
                format!("unable to wait for rugix-ctrl: {err}"),
                None,
            )
            .await;
        }
    }

    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
}

async fn read_output_lines<R>(jobs: JobManager, job_id: String, stream: &'static str, reader: R)
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        jobs.emit_output(&job_id, stream, line).await;
    }
}
