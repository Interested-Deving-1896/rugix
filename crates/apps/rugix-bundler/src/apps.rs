//! App bundle packing logic for Docker Compose, binary, and generic orchestrators.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use reportify::{bail, ResultExt};
use rugix_bundle::manifest::{
    AppArchiveDeliveryConfig, AppFileDeliveryConfig, BlockEncoding, BundleManifest, Compression,
    DeliveryConfig, Payload, UpdateType, XzCompression,
};
use rugix_bundle::{bundle_hash, BundleResult};
use rugix_chunker::ChunkerAlgorithm;
use tracing::info;

/// Normalize non-deterministic tar header fields (timestamps, ownership) for
/// reproducible builds while preserving permission bits.
fn normalize_tar_header(header: &mut tar::Header) {
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_username("").ok();
    header.set_groupname("").ok();
    header.set_cksum();
}

/// Append a file to a tar archive with normalized metadata for reproducibility,
/// preserving the file's permission bits.
fn tar_append_file(archive: &mut tar::Builder<File>, path: &Path, name: &str) -> BundleResult<()> {
    let mut file =
        File::open(path).whatever_with(|_| format!("unable to open {}", path.display()))?;
    let metadata = file
        .metadata()
        .whatever_with(|_| format!("unable to read metadata of {}", path.display()))?;
    let mut header = tar::Header::new_gnu();
    header.set_size(metadata.len());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        header.set_mode(metadata.permissions().mode());
    }
    #[cfg(not(unix))]
    {
        header.set_mode(0o644);
    }
    normalize_tar_header(&mut header);
    archive
        .append_data(&mut header, name, &mut file)
        .whatever_with(|_| format!("unable to add {} to archive", name))?;
    Ok(())
}

/// Append a directory tree to a tar archive with normalized metadata for
/// reproducibility, preserving permission bits and sorting entries for
/// deterministic ordering.
fn tar_append_dir(archive: &mut tar::Builder<File>, name: &str, src: &Path) -> BundleResult<()> {
    fn walk(archive: &mut tar::Builder<File>, prefix: &str, dir: &Path) -> BundleResult<()> {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .whatever_with(|_| format!("unable to read directory {}", dir.display()))?
            .collect::<Result<Vec<_>, _>>()
            .whatever("unable to read directory entry")?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let file_name = entry.file_name();
            let Some(file_name_str) = file_name.to_str() else {
                bail!("non-UTF-8 filename in directory {}", dir.display());
            };
            let entry_name = format!("{}/{}", prefix, file_name_str);
            if path.is_dir() {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                normalize_tar_header(&mut header);
                archive
                    .append_data(&mut header, &entry_name, &[][..])
                    .whatever("unable to add directory entry to archive")?;
                walk(archive, &entry_name, &path)?;
            } else {
                tar_append_file(archive, &path, &entry_name)?;
            }
        }
        Ok(())
    }
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_size(0);
    header.set_mode(0o755);
    normalize_tar_header(&mut header);
    archive
        .append_data(&mut header, name, &[][..])
        .whatever("unable to add directory entry to archive")?;
    walk(archive, name, src)
}

/// Write an `app.toml` entry into a tar archive.
fn tar_append_app_toml(archive: &mut tar::Builder<File>, orchestrator: &str) -> BundleResult<()> {
    let content = format!("orchestrator = \"{orchestrator}\"\n");
    let bytes = content.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    normalize_tar_header(&mut header);
    archive
        .append_data(&mut header, "app.toml", bytes)
        .whatever("unable to add app.toml to archive")?;
    Ok(())
}

/// Append included files/directories to a tar archive.
fn tar_append_includes(archive: &mut tar::Builder<File>, includes: &[PathBuf]) -> BundleResult<()> {
    for include in includes {
        let Some(name) = include.file_name().and_then(|n| n.to_str()) else {
            bail!(
                "unable to determine name for include path: {}",
                include.display()
            );
        };
        if include.is_dir() {
            tar_append_dir(archive, name, include)?;
        } else {
            tar_append_file(archive, include, name)?;
        }
    }
    Ok(())
}

/// Common block encoding configuration for app bundles.
fn app_block_encoding() -> Option<BlockEncoding> {
    Some(
        BlockEncoding::new(ChunkerAlgorithm::Casync {
            avg_block_size_kib: 64,
        })
        .with_compression(Some(Compression::Xz(XzCompression::new()))),
    )
}

/// Write a manifest, pack the bundle, and print the hash.
fn finalize_bundle(
    bundle_dir: &Path,
    output: &Path,
    app: &str,
    payloads: Vec<Payload>,
) -> BundleResult<()> {
    let manifest = BundleManifest::new(UpdateType::Full, payloads);
    fs::write(
        bundle_dir.join("rugix-bundle.toml"),
        toml::to_string_pretty(&manifest).whatever("unable to serialize manifest")?,
    )
    .whatever("unable to write manifest")?;
    rugix_bundle::builder::pack(bundle_dir, output)?;
    let hash = bundle_hash(output)?;
    println!("{hash}");
    info!(app = %app, output = ?output, "packed app bundle");
    Ok(())
}

// ---------------------------------------------------------------------------
// Container image helpers
// ---------------------------------------------------------------------------

/// Extract the string representation from a saphyr YAML node.
fn yaml_as_str<'a>(node: &'a saphyr::Yaml<'a>) -> Option<&'a str> {
    match node {
        saphyr::Yaml::Representation(cow, _, _) => Some(cow.as_ref()),
        saphyr::Yaml::Value(saphyr::Scalar::String(cow)) => Some(cow.as_ref()),
        _ => None,
    }
}

/// Look up a key in a saphyr YAML mapping node.
fn yaml_mapping_get<'a, 'b>(node: &'a saphyr::Yaml<'b>, key: &str) -> Option<&'a saphyr::Yaml<'b>> {
    if let saphyr::Yaml::Mapping(mapping) = node {
        for (k, v) in mapping.iter() {
            if yaml_as_str(k) == Some(key) {
                return Some(v);
            }
        }
    }
    None
}

/// Extract image references from a Docker Compose file.
fn extract_compose_images(compose_path: &Path) -> BundleResult<Vec<String>> {
    let content =
        fs::read_to_string(compose_path).whatever("unable to read Docker Compose file")?;
    use saphyr::LoadableYamlNode;
    let docs =
        saphyr::Yaml::load_from_str(&content).whatever("unable to parse Docker Compose file")?;
    let mut images = Vec::new();
    if let Some(doc) = docs.first() {
        if let Some(saphyr::Yaml::Mapping(services)) = yaml_mapping_get(doc, "services") {
            for (_key, service) in services.iter() {
                if let Some(image_node) = yaml_mapping_get(service, "image") {
                    if let Some(image) = yaml_as_str(image_node) {
                        images.push(image.to_owned());
                    }
                }
            }
        }
    }
    Ok(images)
}

/// Check whether skopeo is available on the system.
fn has_skopeo() -> bool {
    std::process::Command::new("skopeo")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Derive a stable payload filename for a container image by index.
///
/// Uses an index-based name (`image-0.tar`, `image-1.tar`, ...) so that
/// filenames remain stable across version bumps of the same image, enabling
/// effective delta updates between bundles.
fn image_payload_filename(index: usize) -> String {
    format!("image-{index}.tar")
}

/// A saved container image with its payload filename and local path.
struct SavedImage {
    /// Path inside the generation directory (e.g., `images/registry-image-tag.tar`).
    app_path: String,
    /// Filename of the payload file inside the payloads directory.
    payload_filename: String,
}

/// Save container images using skopeo (one tar per image, pulled from registry).
fn save_images_skopeo(
    images: &[String],
    platform: Option<&str>,
    payloads_dir: &Path,
) -> BundleResult<Vec<SavedImage>> {
    let mut saved = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let filename = image_payload_filename(index);
        let output = payloads_dir.join(&filename);
        let mut cmd = std::process::Command::new("skopeo");
        cmd.arg("copy");
        if let Some(platform) = platform {
            // Parse platform string like "linux/arm64" into os and arch.
            let parts: Vec<&str> = platform.splitn(2, '/').collect();
            if parts.len() == 2 {
                cmd.arg("--override-os").arg(parts[0]);
                cmd.arg("--override-arch").arg(parts[1]);
            }
        }
        cmd.arg(format!("docker://{image}"));
        cmd.arg(format!("docker-archive:{}", output.display()));
        info!(image, ?platform, "pulling image with skopeo");
        let status = cmd.status().whatever("unable to run skopeo")?;
        if !status.success() {
            bail!("skopeo copy failed for {image}");
        }
        saved.push(SavedImage {
            app_path: format!("images/{filename}"),
            payload_filename: filename,
        });
    }
    Ok(saved)
}

/// Save container images using `docker pull` + `docker save` (one tar per image).
fn save_images_docker(
    images: &[String],
    platform: Option<&str>,
    pull: bool,
    payloads_dir: &Path,
) -> BundleResult<Vec<SavedImage>> {
    if pull {
        for image in images {
            let mut cmd = std::process::Command::new("docker");
            cmd.arg("pull");
            if let Some(platform) = platform {
                cmd.arg("--platform").arg(platform);
            }
            cmd.arg(image);
            info!(image, ?platform, "pulling docker image");
            let status = cmd.status().whatever("unable to run docker pull")?;
            if !status.success() {
                bail!("docker pull failed for {image}");
            }
        }
    }

    let mut saved = Vec::new();
    for (index, image) in images.iter().enumerate() {
        let filename = image_payload_filename(index);
        let output = payloads_dir.join(&filename);
        info!(image, "saving docker image");
        let mut save_cmd = std::process::Command::new("docker");
        save_cmd.arg("save").arg("-o").arg(&output);
        if let Some(platform) = platform {
            save_cmd.arg("--platform").arg(platform);
        }
        save_cmd.arg(image);
        let status = save_cmd.status().whatever("unable to run docker save")?;
        if !status.success() {
            bail!("docker save failed for {image}");
        }
        saved.push(SavedImage {
            app_path: format!("images/{filename}"),
            payload_filename: filename,
        });
    }
    Ok(saved)
}

/// Save container images to the payloads directory.
///
/// When `pull` is true and skopeo is available, uses skopeo to pull images
/// directly from the registry (one tar per image, no Docker daemon required).
/// Otherwise, falls back to `docker pull` + `docker save`.
fn save_images(
    images: &[String],
    platform: Option<&str>,
    pull: bool,
    payloads_dir: &Path,
) -> BundleResult<Vec<SavedImage>> {
    if pull && has_skopeo() {
        save_images_skopeo(images, platform, payloads_dir)
    } else {
        save_images_docker(images, platform, pull, payloads_dir)
    }
}

/// Pack a Docker Compose app into an app bundle.
///
/// Creates a bundle with:
/// - An `app-archive` payload containing `app.toml`, `docker-compose.yml`, and any extra
///   included files/directories.
/// - An `app-file` payload for the Docker images (saved via `docker save`), placed at
///   `images/images.tar` inside the generation directory.
pub fn pack_docker_compose(cmd: &super::PackDockerComposeCmd) -> BundleResult<()> {
    let bundle_dir = tempfile::TempDir::new().whatever("unable to create temp directory")?;
    let payloads_dir = bundle_dir.path().join("payloads");
    fs::create_dir_all(&payloads_dir).whatever("unable to create payloads directory")?;

    let block_encoding = app_block_encoding();

    // Build the base tar archive: app.toml + docker-compose.yml + includes.
    let archive_path = payloads_dir.join("base.tar");
    {
        let archive_file = File::create(&archive_path).whatever("unable to create base.tar")?;
        let mut archive = tar::Builder::new(archive_file);
        tar_append_app_toml(&mut archive, "docker-compose")?;
        tar_append_file(&mut archive, &cmd.compose_file, "docker-compose.yml")?;
        tar_append_includes(&mut archive, &cmd.includes)?;
        archive.finish().whatever("unable to finish archive")?;
    }

    let mut payloads = vec![Payload {
        delivery: DeliveryConfig::AppArchive(AppArchiveDeliveryConfig::new(cmd.app.clone())),
        filename: "base.tar".to_owned(),
        block_encoding: block_encoding.clone(),
        delta_encoding: None,
    }];

    // Save and add Docker images.
    if !cmd.no_images {
        let images = extract_compose_images(&cmd.compose_file)?;
        if !images.is_empty() {
            let saved = save_images(&images, cmd.platform.as_deref(), cmd.pull, &payloads_dir)?;
            for image in saved {
                payloads.push(Payload {
                    delivery: DeliveryConfig::AppFile(AppFileDeliveryConfig::new(
                        cmd.app.clone(),
                        image.app_path,
                    )),
                    filename: image.payload_filename,
                    block_encoding: block_encoding.clone(),
                    delta_encoding: None,
                });
            }
        }
    }

    finalize_bundle(bundle_dir.path(), &cmd.output, &cmd.app, payloads)
}

/// Pack a binary app into an app bundle.
///
/// Creates a bundle with:
/// - An `app-archive` payload containing `app.toml`, `systemd.service`, and any extra
///   included files/directories.
/// - An `app-file` payload for the binary executable.
pub fn pack_binary(cmd: &super::PackBinaryCmd) -> BundleResult<()> {
    let bundle_dir = tempfile::TempDir::new().whatever("unable to create temp directory")?;
    let payloads_dir = bundle_dir.path().join("payloads");
    fs::create_dir_all(&payloads_dir).whatever("unable to create payloads directory")?;

    let block_encoding = app_block_encoding();

    // Build the base tar archive: app.toml + systemd.service + includes.
    let archive_path = payloads_dir.join("base.tar");
    {
        let archive_file = File::create(&archive_path).whatever("unable to create base.tar")?;
        let mut archive = tar::Builder::new(archive_file);
        tar_append_app_toml(&mut archive, "binary")?;
        tar_append_file(&mut archive, &cmd.service, "systemd.service")?;
        tar_append_includes(&mut archive, &cmd.includes)?;
        archive.finish().whatever("unable to finish archive")?;
    }

    // The binary goes as a separate app-file payload for optimal delta updates.
    let Some(binary_name) = cmd.binary.file_name().and_then(|n| n.to_str()) else {
        bail!(
            "unable to determine binary filename: {}",
            cmd.binary.display()
        );
    };
    fs::copy(&cmd.binary, payloads_dir.join("binary")).whatever("unable to copy binary")?;

    let payloads = vec![
        Payload {
            delivery: DeliveryConfig::AppArchive(AppArchiveDeliveryConfig::new(cmd.app.clone())),
            filename: "base.tar".to_owned(),
            block_encoding: block_encoding.clone(),
            delta_encoding: None,
        },
        Payload {
            delivery: DeliveryConfig::AppFile(AppFileDeliveryConfig::new(
                cmd.app.clone(),
                binary_name.to_owned(),
            )),
            filename: "binary".to_owned(),
            block_encoding: block_encoding.clone(),
            delta_encoding: None,
        },
    ];

    finalize_bundle(bundle_dir.path(), &cmd.output, &cmd.app, payloads)
}

/// Pack a generic app into an app bundle.
///
/// Creates a bundle with:
/// - An `app-archive` payload containing `app.toml`, the `orchestrator` script, and any
///   extra included files/directories.
pub fn pack_generic(cmd: &super::PackGenericCmd) -> BundleResult<()> {
    let bundle_dir = tempfile::TempDir::new().whatever("unable to create temp directory")?;
    let payloads_dir = bundle_dir.path().join("payloads");
    fs::create_dir_all(&payloads_dir).whatever("unable to create payloads directory")?;

    let block_encoding = app_block_encoding();

    // Build the base tar archive: app.toml + orchestrator + includes.
    let archive_path = payloads_dir.join("base.tar");
    {
        let archive_file = File::create(&archive_path).whatever("unable to create base.tar")?;
        let mut archive = tar::Builder::new(archive_file);
        tar_append_app_toml(&mut archive, "generic")?;
        tar_append_file(&mut archive, &cmd.orchestrator, "orchestrator")?;
        tar_append_includes(&mut archive, &cmd.includes)?;
        archive.finish().whatever("unable to finish archive")?;
    }

    let payloads = vec![Payload {
        delivery: DeliveryConfig::AppArchive(AppArchiveDeliveryConfig::new(cmd.app.clone())),
        filename: "base.tar".to_owned(),
        block_encoding,
        delta_encoding: None,
    }];

    finalize_bundle(bundle_dir.path(), &cmd.output, &cmd.app, payloads)
}
