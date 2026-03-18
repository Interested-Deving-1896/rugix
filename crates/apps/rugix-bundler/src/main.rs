use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use cms::cert::x509::der::oid::db::rfc5911::ID_SIGNED_DATA;
use cms::cert::x509::der::Decode;
use reportify::{bail, ResultExt};
use rugix_bundle::format::decode::decode_slice;
use rugix_bundle::format::tags::TagNameResolver;
use rugix_bundle::manifest::{
    AppArchiveDeliveryConfig, AppFileDeliveryConfig, BlockEncoding, BundleManifest, Compression,
    DeliveryConfig, DeltaEncoding, DeltaEncodingFormat, DeltaEncodingInput, HashAlgorithm, Payload,
    UpdateType, XzCompression,
};
use rugix_bundle::reader::BundleReader;
use rugix_bundle::source::FileSource;
use rugix_bundle::xdelta::xdelta_compress;
use rugix_bundle::{add_bundle_signature, bundle_hash, format, signed_metadata, BundleResult};
use rugix_chunker::ChunkerAlgorithm;
use si_crypto_hashes::HashDigest;
use tracing::{info, Level};

mod simulation;

#[derive(Debug, Parser)]
#[clap(version = rugix_version::RUGIX_GIT_VERSION)]
pub struct Args {
    #[clap(subcommand)]
    cmd: Cmd,
    #[clap(flatten)]
    logging: si_observability::clap4::LoggingArgs,
}

#[derive(Debug, Parser)]
pub enum Cmd {
    /// Create a bundle from a bundle directory.
    Bundle(BundleCmd),
    /// Unpack a bundle into a bundle directory.
    Unpack(UnpackCmd),
    /// Hash the header of a bundle.
    Hash(HashCmd),
    /// Extract a payload from a bundle.
    Extract(ExtractCmd),
    /// Compute a static delta update.
    Delta(DeltaCmd),
    /// Inspect an update bundle.
    Inspect(InspectCmd),
    /// App bundle commands.
    #[clap(subcommand)]
    Apps(AppsCmd),
    /// Manipulate and inspect signatures.
    #[clap(subcommand)]
    Signatures(SignaturesCmd),
    /// Simulate an update.
    #[clap(subcommand)]
    Simulator(simulation::SimulationCmd),
    /// Print the low-level structure of a bundle.
    #[clap(hide(true))]
    PrintStructure(PrintCmd),
}

#[derive(Debug, Subcommand)]
pub enum AppsCmd {
    /// Pack an app into a bundle.
    #[clap(subcommand)]
    Pack(AppsPackCmd),
}

#[derive(Debug, Subcommand)]
pub enum AppsPackCmd {
    /// Pack a Docker Compose app into an app bundle.
    DockerCompose(PackDockerComposeCmd),
}

#[derive(Debug, Parser)]
pub struct PackDockerComposeCmd {
    /// App name.
    #[clap(long)]
    app: String,
    /// Target platform for Docker images (e.g., `linux/arm64`, `linux/amd64`).
    /// If not specified, images are saved for the host platform.
    #[clap(long)]
    platform: Option<String>,
    /// Pull images before saving (useful to ensure the latest version or
    /// a specific `--platform` is cached locally).
    #[clap(long)]
    pull: bool,
    /// Skip saving Docker images (by default, images referenced in the compose
    /// file are saved via `docker save` and included in the bundle).
    #[clap(long)]
    no_images: bool,
    /// Extra files or directories to include in the archive.
    /// Each entry is added at the same relative path inside the generation directory.
    #[clap(long = "include")]
    includes: Vec<PathBuf>,
    /// Path to the Docker Compose file.
    compose_file: PathBuf,
    /// Output bundle file.
    output: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum SignaturesCmd {
    /// Add a signature to a bundle.
    Add {
        /// Bundle to add the signature to.
        bundle: PathBuf,
        /// Signature in CMS format.
        signature: PathBuf,
        /// Output bundle.
        out: PathBuf,
    },
    /// Extract bundle metadata for signing.
    Prepare {
        /// Bundle to extract metadata from.
        bundle: PathBuf,
        /// Output path.
        out: PathBuf,
    },
    /// List the signatures in a bundle.
    List {
        /// Bundle to inspect.
        bundle: PathBuf,
    },
    /// Sign a bundle.
    Sign {
        /// Additional intermediate certificates to include.
        #[clap(long = "intermediate-cert")]
        certs: Vec<PathBuf>,
        /// Bundle to sign.
        bundle: PathBuf,
        /// Signer certificate.
        cert: PathBuf,
        /// Signer private key.
        key: PathBuf,
        /// Output path.
        out: PathBuf,
    },
    /// Verify that the bundle has been signed using the given certificate.
    Verify {
        /// Bundle to verify.
        bundle: PathBuf,
        /// Root certificate.
        cert: PathBuf,
    },
}

#[derive(Debug, Parser)]
pub struct PrintCmd {
    /// Path to the update bundle.
    bundle: PathBuf,
}

#[derive(Debug, Parser)]
pub struct BundleCmd {
    /// Source bundle directory.
    src: PathBuf,
    /// Output bundle file.
    dst: PathBuf,
}

#[derive(Debug, Parser)]
pub struct ExtractCmd {
    /// Expected bundle hash to verify while reading.
    #[clap(long)]
    bundle_hash: Option<HashDigest>,
    /// Path to the update bundle.
    bundle: PathBuf,
    /// Index of the payload to extract.
    payload: usize,
    /// Output file path.
    dst: PathBuf,
}

#[derive(Debug, Parser)]
pub struct DeltaCmd {
    /// Slots to compute patches for.
    #[clap(long = "slot")]
    slots: Vec<String>,
    /// Path to the old bundle.
    old: PathBuf,
    /// Path to the new bundle.
    new: PathBuf,
    /// Path to the output patch bundle.
    out: PathBuf,
    /// Disable compression of individual patch blocks.
    #[clap(long)]
    disable_compression: bool,
}

#[derive(Debug, Parser)]
pub struct UnpackCmd {
    /// Path to the bundle.
    src: PathBuf,
    /// Output directory.
    out: PathBuf,
}

#[derive(Debug, Parser)]
pub struct InspectCmd {
    /// Expected bundle hash to verify while reading.
    #[clap(long)]
    bundle_hash: Option<HashDigest>,
    /// Path to the update bundle.
    bundle: PathBuf,
}

#[derive(Debug, Parser)]
pub struct HashCmd {
    /// Path to the update bundle.
    bundle: PathBuf,
}

fn main() -> BundleResult<()> {
    let args = Args::parse();
    let _guard = si_observability::Initializer::new("RUGIX")
        .apply(&args.logging)
        .init();
    match args.cmd {
        Cmd::Bundle(create_cmd) => {
            rugix_bundle::builder::pack(&create_cmd.src, &create_cmd.dst)?;
        }
        Cmd::Unpack(cmd) => {
            unpack(&cmd.src, &cmd.out)?;
        }
        Cmd::Extract(unpack_cmd) => {
            let source = FileSource::from_unbuffered(File::open(&unpack_cmd.bundle).unwrap());
            let mut reader = BundleReader::start(source, unpack_cmd.bundle_hash)?;
            let mut did_read = false;
            while let Some(payload_reader) = reader.next_payload()? {
                if payload_reader.idx() != unpack_cmd.payload {
                    payload_reader.skip()?;
                } else {
                    println!("unpacking payload...");
                    let target = std::fs::OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .read(true)
                        .write(true)
                        .open(&unpack_cmd.dst)
                        .whatever("unable to open payload target")?;
                    payload_reader.decode_into(target, None, &mut |_| {})?;
                    did_read = true;
                    break;
                }
            }
            if !did_read {
                bail!("not enough payloads");
            }
        }
        Cmd::PrintStructure(print_cmd) => {
            let mut source = FileSource::from_unbuffered(File::open(&print_cmd.bundle).unwrap());
            rugix_bundle::format::stlv::pretty_print(&mut source, Some(&TagNameResolver)).unwrap();
        }
        Cmd::Hash(hash_cmd) => {
            let hash = rugix_bundle::bundle_hash(&hash_cmd.bundle).unwrap();
            println!("{hash}");
        }
        Cmd::Inspect(inspect_cmd) => {
            let source = FileSource::from_unbuffered(File::open(&inspect_cmd.bundle).unwrap());
            let reader = BundleReader::start(source, inspect_cmd.bundle_hash)?;
            println!("Payloads:");
            for (idx, entry) in reader.header().payload_index.iter().enumerate() {
                if let Some(slot_type) = &entry.type_slot {
                    println!(
                        "  {idx}: slot={:?} file={}",
                        slot_type.slot,
                        HashDigest::new_unchecked(
                            reader.header().hash_algorithm,
                            &entry.file_hash.raw
                        )
                    );
                }
                if let Some(type_execute) = &entry.type_execute {
                    let command = type_execute.handler.join(" ");
                    println!(
                        "  {idx}: execute({command}) file={}",
                        HashDigest::new_unchecked(
                            reader.header().hash_algorithm,
                            &entry.file_hash.raw
                        )
                    );
                }
                if let Some(type_app_file) = &entry.type_app_file {
                    println!(
                        "  {idx}: app-file app={:?} path={:?} file={}",
                        type_app_file.app,
                        type_app_file.path,
                        HashDigest::new_unchecked(
                            reader.header().hash_algorithm,
                            &entry.file_hash.raw
                        )
                    );
                }
                if let Some(type_app_archive) = &entry.type_app_archive {
                    println!(
                        "  {idx}: app-archive app={:?} file={}",
                        type_app_archive.app,
                        HashDigest::new_unchecked(
                            reader.header().hash_algorithm,
                            &entry.file_hash.raw
                        )
                    );
                }
            }
        }
        Cmd::Apps(apps_cmd) => match apps_cmd {
            AppsCmd::Pack(pack_cmd) => match pack_cmd {
                AppsPackCmd::DockerCompose(cmd) => {
                    pack_docker_compose(&cmd)?;
                }
            },
        },
        Cmd::Delta(cmd) => {
            let old_dir = tempfile::TempDir::new().unwrap();
            info!(directory = ?old_dir.path(), "unpacking old update bundle");
            unpack(&cmd.old, old_dir.path())?;
            let new_dir = tempfile::TempDir::new().unwrap();
            info!(directory = ?new_dir.path(), "unpacking new update bundle");
            unpack(&cmd.new, new_dir.path())?;
            let old_manifest = toml::from_str::<BundleManifest>(
                &std::fs::read_to_string(old_dir.path().join("rugix-bundle.toml")).unwrap(),
            )
            .unwrap();
            let mut new_manifest = toml::from_str::<BundleManifest>(
                &std::fs::read_to_string(new_dir.path().join("rugix-bundle.toml")).unwrap(),
            )
            .unwrap();
            let explicit_slots = !cmd.slots.is_empty();
            let slots = if explicit_slots {
                cmd.slots.as_slice()
            } else {
                &["system".to_owned(), "boot:system".to_owned()]
            };
            for slot in slots {
                let (new_slot, old_slot) = slot
                    .split_once(':')
                    .unwrap_or((slot.as_str(), slot.as_str()));
                let Some(new_payload_idx) =
                    new_manifest
                        .payloads
                        .iter()
                        .position(|p| match &p.delivery {
                            DeliveryConfig::Slot(config) => &config.slot == new_slot,
                            _ => false,
                        })
                else {
                    if explicit_slots {
                        panic!("unable to find slot {new_slot} in new bundle");
                    }
                    continue;
                };
                let Some(old_payload_idx) =
                    old_manifest
                        .payloads
                        .iter()
                        .position(|p| match &p.delivery {
                            DeliveryConfig::Slot(config) => &config.slot == old_slot,
                            _ => false,
                        })
                else {
                    if explicit_slots {
                        panic!("unable to find slot {old_slot} in old bundle");
                    }
                    continue;
                };
                info!(%old_slot, %new_slot, "computing delta");
                let old_filename = &old_manifest.payloads[old_payload_idx].filename;
                let new_filename = &old_manifest.payloads[new_payload_idx].filename;
                let new_filename_patched = format!("{new_filename}.xdelta");
                let old_path = old_dir.path().join("payloads").join(old_filename);
                let new_path = new_dir.path().join("payloads").join(new_filename);
                let hash_algorithm = new_manifest
                    .hash_algorithm
                    .unwrap_or(si_crypto_hashes::HashAlgorithm::Sha512_256);
                let old_hash = hash_file(hash_algorithm, &old_path);
                let new_hash = hash_file(hash_algorithm, &new_path);
                let patch_path = new_dir.path().join("payloads").join(&new_filename_patched);
                xdelta_compress(&old_path, &new_path, &patch_path)?;
                std::fs::remove_file(&new_path).unwrap();
                assert!(patch_path.exists());
                let new_payload = &mut new_manifest.payloads[new_payload_idx];
                new_payload.filename = new_filename_patched;
                new_payload.block_encoding = Some(
                    BlockEncoding::new(ChunkerAlgorithm::Fixed {
                        block_size_kib: 256,
                    })
                    .with_compression(if cmd.disable_compression {
                        None
                    } else {
                        Some(Compression::Xz(XzCompression::new()))
                    }),
                );
                new_payload.delta_encoding = Some(DeltaEncoding::new(
                    vec![DeltaEncodingInput {
                        hashes: vec![old_hash],
                    }],
                    DeltaEncodingFormat::Xdelta,
                    new_hash,
                ));
            }
            // Compute deltas for app-file payloads with matching paths.
            for new_payload_idx in 0..new_manifest.payloads.len() {
                let DeliveryConfig::AppFile(ref new_config) =
                    new_manifest.payloads[new_payload_idx].delivery
                else {
                    continue;
                };
                let new_app = new_config.app.clone();
                let new_app_path = new_config.path.clone();
                let Some(old_payload_idx) = old_manifest.payloads.iter().position(|p| {
                    matches!(
                        &p.delivery,
                        DeliveryConfig::AppFile(config)
                            if config.app == new_app && config.path == new_app_path
                    )
                }) else {
                    info!(app = %new_app, path = %new_app_path, "no matching app-file in old bundle, skipping");
                    continue;
                };
                info!(app = %new_app, path = %new_app_path, "computing app-file delta");
                let old_filename = &old_manifest.payloads[old_payload_idx].filename;
                let new_filename = &new_manifest.payloads[new_payload_idx].filename;
                let new_filename_patched = format!("{new_filename}.xdelta");
                let old_path = old_dir.path().join("payloads").join(old_filename);
                let new_path = new_dir.path().join("payloads").join(new_filename);
                let hash_algorithm = new_manifest
                    .hash_algorithm
                    .unwrap_or(si_crypto_hashes::HashAlgorithm::Sha512_256);
                let old_hash = hash_file(hash_algorithm, &old_path);
                let new_hash = hash_file(hash_algorithm, &new_path);
                let patch_path = new_dir.path().join("payloads").join(&new_filename_patched);
                xdelta_compress(&old_path, &new_path, &patch_path)?;
                std::fs::remove_file(&new_path).unwrap();
                assert!(patch_path.exists());
                let new_payload = &mut new_manifest.payloads[new_payload_idx];
                new_payload.filename = new_filename_patched;
                new_payload.block_encoding = Some(
                    BlockEncoding::new(ChunkerAlgorithm::Fixed {
                        block_size_kib: 256,
                    })
                    .with_compression(if cmd.disable_compression {
                        None
                    } else {
                        Some(Compression::Xz(XzCompression::new()))
                    }),
                );
                new_payload.delta_encoding = Some(DeltaEncoding::new(
                    vec![DeltaEncodingInput {
                        hashes: vec![old_hash],
                    }],
                    DeltaEncodingFormat::Xdelta,
                    new_hash,
                ));
            }
            std::fs::write(
                new_dir.path().join("rugix-bundle.toml"),
                toml::to_string(&new_manifest).unwrap(),
            )
            .unwrap();
            rugix_bundle::builder::pack(new_dir.path(), &cmd.out)?;
        }
        Cmd::Simulator(cmd) => {
            simulation::run(&cmd);
        }
        Cmd::Signatures(cmd) => match cmd {
            SignaturesCmd::Add {
                bundle,
                signature,
                out,
            } => {
                let signature = std::fs::read(&signature).whatever("unable to read signature")?;
                let content_info = cms::content_info::ContentInfo::from_der(&signature)
                    .expect("invalid signature");
                if content_info.content_type != ID_SIGNED_DATA {
                    bail!("invalid signature content type");
                }
                let signed_data = content_info
                    .content
                    .decode_as::<cms::signed_data::SignedData>()
                    .expect("invalid signature");
                println!("CMS Version: {:?}", signed_data.version);
                println!(
                    "Embedded Certificates: {}",
                    signed_data.certificates.map(|c| c.0.len()).unwrap_or(0)
                );
                let bundle_hash = bundle_hash(&bundle)?;
                if let Some(content) = signed_data.encap_content_info.econtent {
                    let signed_metadata = decode_slice::<format::SignedMetadata>(content.value())?;
                    if bundle_hash != signed_metadata.header_hash {
                        bail!("bundle hash does not match signature");
                    }
                } else {
                    bail!("no encapsulated content");
                }
                add_bundle_signature(&bundle, signature, &out)?;
            }
            SignaturesCmd::List { bundle } => {
                let source = FileSource::from_unbuffered(File::open(&bundle).unwrap());
                let reader = BundleReader::start(source, None)?;
                if let Some(signatures) = reader.signatures() {
                    for (idx, signature) in signatures.cms_signatures.iter().enumerate() {
                        println!("CMS Signature {} (length={})", idx, signature.raw.len());
                    }
                } else {
                    println!("No signatures found");
                }
            }
            SignaturesCmd::Prepare { bundle, out } => {
                let metadata = signed_metadata(&bundle)?;
                std::fs::write(out, metadata).whatever("unable to write metadata")?;
            }
            SignaturesCmd::Sign {
                certs,
                bundle,
                cert,
                key,
                out,
            } => {
                let metadata = signed_metadata(&bundle)?;
                let cert_pem =
                    std::fs::read(&cert).whatever("unable to read signer certificate")?;
                let key_pem = std::fs::read(&key).whatever("unable to read private key")?;
                let mut builder = rugix_pki::CmsSignerBuilder::new(&cert_pem, &key_pem)
                    .whatever("unable to create CMS signer")?;
                for cert in certs {
                    let cert_pem =
                        std::fs::read(&cert).whatever("unable to read intermediate certificate")?;
                    builder = builder
                        .with_intermediate_cert(&cert_pem)
                        .whatever("unable to add intermediate certificate")?;
                }
                let signer = builder.build().whatever("unable to build CMS signer")?;
                let signature = signer.sign(&metadata).whatever("unable to sign bundle")?;
                add_bundle_signature(&bundle, signature, &out)?;
            }
            SignaturesCmd::Verify { bundle, cert } => {
                let source = FileSource::from_unbuffered(File::open(&bundle).unwrap());
                let reader = BundleReader::start(source, None)?;
                let Some(signatures) = reader.signatures() else {
                    bail!("no signatures found");
                };
                let cert_pem = std::fs::read(&cert).whatever("unable to read root certificate")?;
                let verifier = rugix_pki::CmsVerifier::new(&cert_pem)
                    .whatever("unable to create CMS verifier")?;
                let mut found_valid_signature = false;
                for signature in signatures.cms_signatures.iter() {
                    let result = match verifier.verify(&signature.raw) {
                        Ok(result) => result,
                        Err(error) => {
                            println!("{error}");
                            continue;
                        }
                    };
                    let signed_metadata = decode_slice::<format::SignedMetadata>(&result.content)
                        .whatever("unable to decode signed metadata")?;
                    if signed_metadata.header_hash
                        == reader.header_hash(signed_metadata.header_hash.algorithm())
                    {
                        found_valid_signature = true;
                        println!("Found valid signature!");
                        break;
                    }
                }
                if !found_valid_signature {
                    bail!("no valid signature found");
                }
            }
        },
    }
    Ok(())
}

pub fn unpack(src: &Path, dst: &Path) -> BundleResult<()> {
    std::fs::create_dir_all(dst).unwrap();
    let source = FileSource::from_unbuffered(File::open(&src).unwrap());
    let mut reader = BundleReader::start(source, None)?;
    let Some(manifest) = &reader.header().manifest else {
        panic!("unpacking requires a manifest");
    };
    let manifest = serde_json::from_str::<BundleManifest>(&manifest).unwrap();
    std::fs::write(
        dst.join("rugix-bundle.toml"),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let payload_dir = dst.join("payloads");
    std::fs::create_dir_all(&payload_dir).unwrap();
    while let Some(payload_reader) = reader.next_payload()? {
        let filename = &manifest.payloads[payload_reader.idx()].filename;
        info!(%filename, "unpacking bundle payload");
        let target = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(payload_dir.join(filename))
            .whatever("unable to open payload target")?;
        payload_reader.decode_into(target, None, &mut |_| {})?;
    }
    Ok(())
}

#[tracing::instrument(level = Level::DEBUG)]
pub fn hash_file(algorithm: HashAlgorithm, path: &Path) -> HashDigest {
    let mut file = std::fs::File::open(&path).unwrap();
    let mut buffer = vec![0u8; 8096];
    let mut hasher = algorithm.hasher();
    loop {
        let chunk_size = file.read(&mut buffer).unwrap();
        if chunk_size > 0 {
            hasher.update(&buffer[..chunk_size]);
        } else {
            break;
        }
    }
    hasher.finalize()
}

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

/// Save Docker images via `docker save`.
///
/// When `pull` is true, each image is pulled first (with the specified platform if
/// given).
fn docker_save(
    images: &[String],
    platform: Option<&str>,
    pull: bool,
    output: &Path,
) -> BundleResult<()> {
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

    info!(?images, "saving docker images");
    let mut save_cmd = std::process::Command::new("docker");
    save_cmd.arg("save").arg("-o").arg(output);
    if let Some(platform) = platform {
        save_cmd.arg("--platform").arg(platform);
    }
    save_cmd.args(images);
    let status = save_cmd.status().whatever("unable to run docker save")?;
    if !status.success() {
        bail!("docker save failed");
    }
    Ok(())
}

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

/// Pack a Docker Compose app into an app bundle.
///
/// Creates a bundle with:
/// - An `app-archive` payload containing `app.toml`, `docker-compose.yml`, and any extra
///   included files/directories.
/// - An `app-file` payload for the Docker images (saved via `docker save`), placed at
///   `images/images.tar` inside the generation directory.
fn pack_docker_compose(cmd: &PackDockerComposeCmd) -> BundleResult<()> {
    let bundle_dir = tempfile::TempDir::new().whatever("unable to create temp directory")?;
    let payloads_dir = bundle_dir.path().join("payloads");
    fs::create_dir_all(&payloads_dir).whatever("unable to create payloads directory")?;

    // Build the base tar archive: app.toml + docker-compose.yml + includes.
    let archive_path = payloads_dir.join("base.tar");
    {
        let archive_file = File::create(&archive_path).whatever("unable to create base.tar")?;
        let mut archive = tar::Builder::new(archive_file);

        // app.toml
        let app_toml = b"orchestrator = \"docker-compose\"\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(app_toml.len() as u64);
        header.set_mode(0o644);
        normalize_tar_header(&mut header);
        archive
            .append_data(&mut header, "app.toml", &app_toml[..])
            .whatever("unable to add app.toml to archive")?;

        // docker-compose.yml
        tar_append_file(&mut archive, &cmd.compose_file, "docker-compose.yml")?;

        // Extra files and directories.
        for include in &cmd.includes {
            let Some(name) = include.file_name().and_then(|n| n.to_str()) else {
                bail!(
                    "unable to determine name for include path: {}",
                    include.display()
                );
            };
            if include.is_dir() {
                tar_append_dir(&mut archive, name, include)?;
            } else {
                tar_append_file(&mut archive, include, name)?;
            }
        }

        archive.finish().whatever("unable to finish archive")?;
    }

    let block_encoding = Some(
        BlockEncoding::new(ChunkerAlgorithm::Casync {
            avg_block_size_kib: 64,
        })
        .with_compression(Some(Compression::Xz(XzCompression::new()))),
    );

    // Build the manifest.
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
            let image_payload = payloads_dir.join("images.tar");
            docker_save(&images, cmd.platform.as_deref(), cmd.pull, &image_payload)?;
            payloads.push(Payload {
                delivery: DeliveryConfig::AppFile(AppFileDeliveryConfig::new(
                    cmd.app.clone(),
                    "images/images.tar".to_owned(),
                )),
                filename: "images.tar".to_owned(),
                block_encoding: block_encoding.clone(),
                delta_encoding: None,
            });
        }
    }

    let manifest = BundleManifest::new(UpdateType::Full, payloads);
    fs::write(
        bundle_dir.path().join("rugix-bundle.toml"),
        toml::to_string_pretty(&manifest).whatever("unable to serialize manifest")?,
    )
    .whatever("unable to write manifest")?;

    rugix_bundle::builder::pack(bundle_dir.path(), &cmd.output)?;
    info!(app = %cmd.app, output = ?cmd.output, "packed compose app bundle");
    Ok(())
}
