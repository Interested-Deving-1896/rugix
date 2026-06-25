use std::fs;
use std::io;
use std::path::Path;

fn main() {
    sidex_build_rs::configure()
        .with_bundle(".")
        .generate()
        .expect("failed to generate Rugix Admin Sidex Rust types");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let frontend_out = Path::new(&out_dir).join("frontend-dist");
    let frontend_dist = Path::new("frontend").join("dist");

    fs::remove_dir_all(&frontend_out).ok();
    fs::create_dir_all(&frontend_out).expect("unable to create frontend output directory");

    if frontend_dist.join("index.html").exists() {
        copy_dir(&frontend_dist, &frontend_out).expect("unable to copy frontend build output");
        emit_rerun_if_changed(&frontend_dist).expect("unable to emit frontend change directives");
    } else {
        println!(
            "cargo:warning=rugix-admin frontend/dist is missing; embedding a minimal placeholder"
        );
        fs::write(
            frontend_out.join("index.html"),
            r#"<!doctype html><html><head><meta charset="utf-8"><title>Rugix Admin</title></head><body><h1>Rugix Admin</h1><p>The frontend has not been built. Run <code>pnpm install --frozen-lockfile</code> and <code>pnpm run build</code> in <code>crates/apps/rugix-admin/frontend</code>, then rebuild <code>rugix-admin</code>.</p></body></html>"#,
        )
        .expect("unable to write placeholder frontend");
    }

    println!("cargo:rerun-if-changed=frontend/dist");
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn emit_rerun_if_changed(path: &Path) -> io::Result<()> {
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            emit_rerun_if_changed(&entry?.path())?;
        }
    }
    Ok(())
}
