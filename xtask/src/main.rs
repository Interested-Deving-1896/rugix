use std::path::PathBuf;

use clap::Parser;
use xscript::{read_str, run, LocalEnv, Out, Run};

#[derive(Debug, Parser)]
pub struct Args {
    #[clap(subcommand)]
    task: Task,
}

#[derive(Debug, Parser)]
pub enum Task {
    Doc,
    Build {
        #[clap(long)]
        no_asm: bool,
    },
    BuildBinaries {
        target: Option<String>,
        #[clap(long)]
        no_asm: bool,
    },
}

pub fn project_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path
}

pub fn get_target_dir() -> PathBuf {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        target_dir.into()
    } else {
        project_path().join("target")
    }
}

pub fn build_binaries(target: &str, no_asm: bool) -> anyhow::Result<()> {
    let mut env = LocalEnv::new(project_path());
    let git_version = read_str!(env, ["git", "describe", "--tags", "--always"])?;
    env.set_var("RUGIX_GIT_VERSION", git_version.trim());
    if no_asm {
        env.set_var("AWS_LC_SYS_NO_ASM", "1");
        run!(
            env,
            [
                "cargo",
                "build",
                "--release",
                "--target",
                target,
                "--bin",
                "rugix-*",
                "--bin",
                "rugix-*",
                "--config",
                "profile.release.package.aws-lc-sys.opt-level=0",
            ]
            .with_stdout(Out::Inherit)
            .with_stderr(Out::Inherit)
        )?;
    } else {
        run!(
            env,
            [
                "cargo",
                "build",
                "--release",
                "--target",
                target,
                "--bin",
                "rugix-*",
                "--bin",
                "rugix-*",
            ]
            .with_stdout(Out::Inherit)
            .with_stderr(Out::Inherit)
        )?;
    }
    let suffix = if no_asm { "-noasm" } else { "" };
    let dir_name = format!("{target}{suffix}");
    let binaries_dir = project_path().join("build/binaries").join(dir_name);
    if binaries_dir.exists() {
        std::fs::remove_dir_all(&binaries_dir)?;
    }
    std::fs::create_dir_all(&binaries_dir)?;
    let target_dir = get_target_dir().join(target).join("release");
    for entry in std::fs::read_dir(&target_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            continue;
        }
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        if !(file_name.starts_with("rugix-") || file_name.starts_with("rugix-"))
            || file_name.ends_with(".d")
        {
            continue;
        }
        std::fs::copy(entry.path(), binaries_dir.join(file_name))?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let env = LocalEnv::new(project_path());
    match args.task {
        Task::Doc => {
            run!(
                env,
                ["cargo", "+nightly", "doc", "--document-private-items",]
                    .with_stdout(Out::Inherit)
                    .with_stderr(Out::Inherit)
            )?;
        }
        Task::BuildBinaries { target, no_asm } => {
            let target = target.as_deref().unwrap_or("aarch64-unknown-linux-musl");
            build_binaries(target, no_asm)?;
        }
        Task::Build { no_asm } => {
            build_binaries("aarch64-unknown-linux-musl", no_asm)?;
            build_binaries("x86_64-unknown-linux-musl", no_asm)?;
        }
    }
    Ok(())
}
