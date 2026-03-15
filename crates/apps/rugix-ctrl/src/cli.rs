//! Definition of the command line interface (CLI).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use std::time::Duration;

use rugix_bundle::format;
use rugix_bundle::format::decode::decode_slice;
use rugix_bundle::manifest::ChunkerAlgorithm;
use rugix_bundle::reader::block_provider::StoredBlockProvider;
use rugix_bundle::reader::{BundleReader, DecodedPayloadInfo, PayloadTarget};
use rugix_bundle::source::{BundleSource, ReaderSource, SkipRead, SkipSeek};
use rugix_bundle::xdelta::xdelta_decompress;
use rugix_cli::widgets::{ProgressBar, ProgressSpinner, Widget};
use rugix_cli::StatusSegment;
use rugix_common::disk::blkdev::{find_block_device, BlockDevice};
use rugix_common::mount::is_mount_point;
use rugix_common::pipe::{buffered_pipe, PipeWriter};
use rugix_common::slots::SlotState;
use rugix_hooks::{HooksLoader, RunOptions};
use si_crypto_hashes::{HashAlgorithm, HashDigest, Hasher};
use tracing::{debug, error, info, trace, warn};

use crate::config::config::Config;
use crate::config::events::{Event, UpdateProgressEvent};
use crate::config::load_ctrl_config;
use crate::system::boot_groups::{BootGroup, BootGroupIdx};
use crate::system::slots::SlotKind;
use crate::system::{System, SystemResult};
use clap::{Parser, ValueEnum};
use reportify::{bail, whatever, ErrorExt, ResultExt};
use rugix_common::stream_hasher::StreamHasher;
use xscript::{vars, Vars};

use crate::config::output::BlockDeviceInfo;
use crate::http_source::{HttpSource, RetryConfig};
use crate::overlay::overlay_dir;
use crate::payload_db::{self, BlockProvider};
use crate::system_state;
use crate::utils::{clear_flag, reboot, set_flag, DEFERRED_SPARE_REBOOT_FLAG};

fn create_rugix_state_directory() -> SystemResult<()> {
    fs::create_dir_all("/run/rugix/state/.rugix")
        .whatever("unable to create `/run/rugix/state/.rugix`")
}

fn set_rugix_state_flag(name: &str, value: Option<&str>) -> SystemResult<()> {
    fs::write(
        Path::new("/run/rugix/state/.rugix").join(name),
        value.unwrap_or_default(),
    )
    .whatever("unable to write state flag")
    .with_info(|_| format!("name: {name}"))
}

fn clear_rugix_state_flag(name: &str) -> SystemResult<()> {
    let path = Path::new("/run/rugix/state/.rugix").join(name);
    fs::remove_file(&path).or_else(|error| match error.kind() {
        io::ErrorKind::NotFound => Ok(()),
        _ => Err(error
            .whatever("unable to clear state flag")
            .with_info(format!("name: {name}"))),
    })?;
    if path.exists() {
        return Err(whatever!("unable to clear state flag").with_info(format!("name: {name}")));
    }
    Ok(())
}

pub fn main() -> SystemResult<()> {
    rugix_cli::CliBuilder::new().init();

    let args = Args::parse();
    let config = load_ctrl_config()?;

    match &args.command {
        Command::State(state_cmd) => match state_cmd {
            StateCommand::Reset {
                backup,
                backup_name,
            } => {
                if backup_name.is_some() && !*backup {
                    warn!("ignoring `--backup-name` option because `--backup` is not set");
                }

                let reset_hooks = HooksLoader::default()
                    .load_hooks("state-reset")
                    .whatever("unable to load `state-reset` hooks")?;
                reset_hooks
                    .run_hooks("prepare", Vars::new(), &Default::default())
                    .whatever("unable to run `state-reset/prepare` hooks")?;
                create_rugix_state_directory()?;
                if *backup {
                    let backup_name = backup_name.clone().unwrap_or_else(|| {
                        jiff::Timestamp::now()
                            .strftime("default.%Y%m%d%H%M%S")
                            .to_string()
                    });
                    set_rugix_state_flag("reset-state", Some(&backup_name))?;
                } else {
                    set_rugix_state_flag("reset-state", None)?;
                };
                reboot()?;
            }
            StateCommand::Overlay(overlay_cmd) => match overlay_cmd {
                OverlayCommand::ForcePersist { persist } => match persist {
                    Boolean::True => {
                        create_rugix_state_directory()?;
                        set_rugix_state_flag("force-persist-overlay", None)?;
                    }
                    Boolean::False => {
                        clear_rugix_state_flag("force-persist-overlay")?;
                    }
                },
            },
        },
        Command::Update(update_cmd) => {
            match update_cmd {
                UpdateCommand::Install {
                    bundle,
                    insecure_skip_bundle_verification,
                    root_cert,
                    bundle_hash,
                    reboot: reboot_type,
                    keep_overlay,
                    boot_group,
                    disable_range_queries,
                    http_max_retries,
                    http_retry_initial_backoff,
                    http_retry_max_backoff,
                } => {
                    let system = System::initialize()?;

                    if system.needs_commit()? {
                        bail!("system needs to be committed before installing an update");
                    }

                    // Find the entry where we are going to install the update to.
                    let boot_group = match boot_group {
                        Some(entry_name) => {
                            let Some(entry) = system.boot_entries().find_by_name(entry_name) else {
                                bail!("unable to find boot group {entry_name}")
                            };
                            Some(entry)
                        }
                        None => {
                            if system.boot_entries().iter().count() > 2 {
                                None
                            } else {
                                system
                                    .boot_entries()
                                    .iter()
                                    .find(|(_, entry)| !entry.active())
                            }
                        }
                    };
                    if let Some((_, boot_group)) = boot_group {
                        info!("installing update to boot group {:?}", boot_group.name());
                        if boot_group.active() {
                            bail!("selected boot group {} is active", boot_group.name());
                        }
                    }

                    let hooks = HooksLoader::default()
                        .load_hooks("update-install")
                        .whatever("unable to load `update-install` hooks")?;

                    let hook_vars = vars! {
                        RUGIX_BOOT_GROUP = boot_group.map(|g| g.1.name()).unwrap_or(""),
                    };

                    hooks
                        .run_hooks("pre-update", hook_vars.clone(), &Default::default())
                        .whatever("error running `pre-update` hooks")?;

                    if !keep_overlay {
                        if let Some(boot_group) = &boot_group {
                            let spare_overlay_dir = overlay_dir(boot_group.1);
                            fs::remove_dir_all(spare_overlay_dir).ok();
                        }
                    }

                    let retry_config = RetryConfig {
                        max_retries: *http_max_retries,
                        initial_backoff: Duration::from_secs(*http_retry_initial_backoff),
                        max_backoff: Duration::from_secs(*http_retry_max_backoff),
                    };
                    let should_reboot = install_update_stream(
                        &system,
                        &config,
                        bundle,
                        boot_group.as_ref(),
                        bundle_hash,
                        root_cert.as_deref(),
                        *insecure_skip_bundle_verification,
                        *disable_range_queries,
                        retry_config,
                    )?;

                    hooks
                        .run_hooks("post-update", hook_vars.clone(), &Default::default())
                        .whatever("error running `post-update` hooks")?;

                    let reboot_type = reboot_type.clone().unwrap_or(should_reboot);

                    match reboot_type {
                        UpdateRebootType::Yes => {
                            let (entry_idx, boot_group) = boot_group.unwrap();
                            info!(
                                "instructing boot flow to try booting into {:?}",
                                boot_group.name()
                            );
                            system
                                .boot_flow()
                                .set_try_next(&system, entry_idx)
                                .whatever("unable to set next boot group")?;
                            info!("rebooting");
                            reboot()?;
                        }
                        UpdateRebootType::No => { /* nothing to do */ }
                        UpdateRebootType::Set => {
                            let (entry_idx, boot_group) = boot_group.unwrap();
                            info!(
                                "instructing boot flow to try booting into {:?}",
                                boot_group.name()
                            );
                            system
                                .boot_flow()
                                .set_try_next(&system, entry_idx)
                                .whatever("unable to set next boot group")?;
                        }
                        UpdateRebootType::Deferred => {
                            set_flag(DEFERRED_SPARE_REBOOT_FLAG)?;
                        }
                    }
                }
            }
        }
        Command::System(sys_cmd) => match sys_cmd {
            SystemCommand::Info { json } => {
                let system = System::initialize()?;
                let output = system_state::state_from_system(&system);
                rugix_cli::json::print_json(&output, *json)
                    .whatever("unable to write system info to stdout")?;
            }
            SystemCommand::Commit => {
                let system = System::initialize()?;

                if system.needs_commit()? {
                    let hooks = HooksLoader::default()
                        .load_hooks("system-commit")
                        .whatever("unable to load `system-commit` hooks")?;
                    hooks
                        .run_hooks("pre-commit", Vars::new(), &Default::default())
                        .whatever("unable to run `pre-commit` hooks")?;
                    system.commit()?;
                    hooks
                        .run_hooks("post-commit", Vars::new(), &Default::default())
                        .whatever("unable to run `post-commit` hooks")?;
                } else {
                    info!("active boot group is already the default");
                }
            }
            SystemCommand::Reboot { spare } => {
                let system = System::initialize()?;
                if *spare {
                    if let Some((spare, _)) = system.spare_entry()? {
                        system
                            .boot_flow()
                            .set_try_next(&system, spare)
                            .whatever("unable to set next boot group")?;
                    }
                }
                reboot()?;
            }
        },
        Command::Unstable(command) => match command {
            UnstableCommand::SetDeferredSpareReboot { value } => match value {
                Boolean::True => set_flag(DEFERRED_SPARE_REBOOT_FLAG)?,
                Boolean::False => clear_flag(DEFERRED_SPARE_REBOOT_FLAG)?,
            },
            UnstableCommand::PrintSystemInfo => {
                let system = System::initialize()?;
                eprintln!("Config:");
                eprintln!("{:#?}", system.config());
                eprintln!("Root:");
                eprintln!("{:#?}", system.root());
                eprintln!("Slots:");
                for (_, slot) in system.slots().iter() {
                    eprintln!("{:#?}", slot)
                }
                eprintln!("Boot Entries");
                eprintln!("{:#?}", system.boot_entries());
            }
        },
        Command::Slots(slots_command) => match slots_command {
            SlotsCommand::Inspect { slot } => {
                let indices = payload_db::get_stored_indices(slot)?;
                #[derive(serde::Serialize)]
                struct SlotInspectOutput<'a> {
                    indices: &'a [payload_db::StoredBlockIndex],
                }
                rugix_cli::json::print_json(&SlotInspectOutput { indices: &indices }, false)
                    .whatever("unable to write slot info to stdout")?;
            }
            SlotsCommand::CreateIndex {
                slot,
                chunker: chunker_algorithm,
                hash_algorithm,
            } => {
                let system = System::initialize()?;
                let Some((_, slot)) = system.slots().find_by_name(slot) else {
                    bail!("slot {slot} not found")
                };
                match slot.kind() {
                    SlotKind::Block(block_slot) => {
                        payload_db::add_index(
                            slot.name(),
                            block_slot.device().path(),
                            chunker_algorithm,
                            hash_algorithm,
                        )?;
                    }
                    SlotKind::File { path } => {
                        payload_db::add_index(
                            slot.name(),
                            path,
                            chunker_algorithm,
                            hash_algorithm,
                        )?;
                    }
                    SlotKind::Custom { .. } => {
                        bail!("cannot create indices on custom slots");
                    }
                }
            }
            SlotsCommand::Verify { slot } => {
                let system = System::initialize()?;
                let Some((_, slot)) = system.slots().find_by_name(slot) else {
                    bail!("slot {slot} not found")
                };
                let Some(slot_state) = payload_db::get_stored_state(slot.name())? else {
                    bail!("no stored state for slot {}", slot.name());
                };
                if !slot.is_immutable() {
                    bail!("slot {} is not immutable, cannot verify", slot.name());
                }
                let Some((_, hash)) = &slot_state.hashes.iter().next() else {
                    bail!("no hashes stored for slot {}", slot.name());
                };
                let mut hasher = hash.algorithm().hasher();
                let mut file = match slot.kind() {
                    SlotKind::Block(block_slot) => {
                        File::open(block_slot.device()).whatever("error opening block device")?
                    }
                    SlotKind::File { path } => File::open(path).whatever("error opening file")?,
                    SlotKind::Custom { .. } => {
                        bail!("cannot create indices on custom slots");
                    }
                };
                info!(expected_hash = %hash, slot_name = slot.name(), size = slot_state.size.map(|s| s.raw), "verifying slot");
                let mut buffer = [0; 4096];
                let mut remaining = slot_state.size.map(|s| s.raw).unwrap_or(u64::MAX);
                let mut bytes_hashed = 0;
                while remaining > 0 {
                    let read = file.read(&mut buffer).whatever("error reading slot file")?;
                    if read == 0 {
                        break;
                    }
                    let chunk = &buffer[..(read as u64).min(remaining) as usize];
                    hasher.update(chunk);
                    bytes_hashed += chunk.len() as u64;
                    remaining = remaining.saturating_sub(read as u64);
                }
                debug!("hashed {} bytes from slot {}", bytes_hashed, slot.name());
                let found = hasher.finalize();
                if found != **hash {
                    bail!(
                        "hash mismatch for slot {}: expected {}, found {}",
                        slot.name(),
                        hash,
                        found
                    );
                }
                info!(slot_name = slot.name(), "slot verified successfully");
            }
        },
        Command::Boot(cmd) => match cmd {
            BootCommand::MarkGood { group } => {
                let system = System::initialize()?;
                let boot_group = match group {
                    Some(entry_name) => {
                        let Some((group, _)) = system.boot_entries().find_by_name(entry_name)
                        else {
                            bail!("unable to find boot group {entry_name}")
                        };
                        group
                    }
                    None => system.active_boot_entry().unwrap(),
                };
                info!(
                    "marking boot group {} as good",
                    system.boot_entries()[boot_group].name()
                );
                system
                    .boot_flow()
                    .mark_good(&system, boot_group)
                    .whatever("unable to mark boot group as good")?;
            }
            BootCommand::MarkBad { group } => {
                let system = System::initialize()?;
                let Some((group, _)) = system.boot_entries().find_by_name(group) else {
                    bail!("unable to find boot group {group}")
                };
                info!(
                    "marking boot group {} as bad",
                    system.boot_entries()[group].name()
                );
                system
                    .boot_flow()
                    .mark_bad(&system, group)
                    .whatever("unable to mark boot group as bad")?;
            }
        },
        Command::Utils(cmd) => match cmd {
            UtilsCommand::FindBlockDevice { path } => {
                let Some(device) =
                    find_block_device(path).whatever("error finding block device")?
                else {
                    bail!("unable to find block device");
                };
                rugix_cli::json::print_json(
                    &BlockDeviceInfo {
                        device: device.path().to_string_lossy().into_owned(),
                        parent: device
                            .find_parent()
                            .ok()
                            .flatten()
                            .map(|parent| parent.path().to_string_lossy().into_owned()),
                        partition: device.is_partition().ok().flatten(),
                    },
                    false,
                )
                .whatever("unable to write block device info to stdout")?;
            }
            UtilsCommand::IsMountPoint { path } => {
                rugix_cli::json::print_json(&is_mount_point(path), false)
                    .whatever("unable to write mount point info to stdout")?;
            }
            UtilsCommand::ResolvePartition { disk, partition } => {
                let system = System::initialize()?;
                let disk = if let Some(disk) = disk {
                    BlockDevice::new(disk).whatever("unable to open disk")?
                } else {
                    system.root().as_ref().unwrap().device.clone()
                };
                let Some(device) = disk
                    .get_partition(*partition)
                    .whatever("unable to get partition")?
                else {
                    bail!("partition not found");
                };
                rugix_cli::json::print_json(
                    &BlockDeviceInfo {
                        device: device.path().to_string_lossy().into_owned(),
                        parent: device
                            .find_parent()
                            .ok()
                            .flatten()
                            .map(|parent| parent.path().to_string_lossy().into_owned()),
                        partition: device.is_partition().ok().flatten(),
                    },
                    false,
                )
                .whatever("unable to write partition info to stdout")?;
            }
        },
        Command::Apps(cmd) => {
            warn!("edge application orchestration is experimental");
            let apps_config =
                crate::apps::config::load_apps_config().whatever("unable to load apps config")?;
            let service_manager = crate::apps::config::effective_service_manager(&apps_config);
            let apps_dir = crate::apps::config::apps_dir().to_owned();
            let manager = crate::apps::manager::AppManager::new(apps_dir, service_manager);
            match cmd {
                AppsCommand::Install {
                    bundle,
                    insecure_skip_bundle_verification,
                } => {
                    install_app_bundle(
                        &config,
                        &manager,
                        bundle,
                        *insecure_skip_bundle_verification,
                    )?;
                }
                AppsCommand::List => {
                    use crate::config::output::{AppListEntryOutput, AppListOutput};
                    let apps = manager.list_apps().whatever("unable to list apps")?;
                    let entries = apps
                        .iter()
                        .map(|app| {
                            let status = manager
                                .app_status(app)
                                .map(|s| format!("{s}"))
                                .unwrap_or_else(|_| "unknown".to_owned());
                            let generation = manager.current_generation(app);
                            (
                                app.clone(),
                                AppListEntryOutput::new(status).with_generation(generation),
                            )
                        })
                        .collect();
                    rugix_cli::json::print_json(&AppListOutput::new(entries), false)
                        .whatever("unable to write apps list to stdout")?;
                }
                AppsCommand::Info { app } => {
                    use crate::config::output::{
                        AppInfoOutput, AppStateActiveOutput, AppStateErrorOutput, AppStateOutput,
                        AppStateSwitchingOutput, GenerationInfoOutput,
                    };
                    let status = manager
                        .app_status(app)
                        .map(|s| format!("{s}"))
                        .unwrap_or_else(|_| "unknown".to_owned());
                    let generations = manager
                        .list_generations(app)
                        .whatever("unable to list generations")?;
                    let current = manager.current_generation(app);
                    let state = manager.read_state(app);
                    let state_output = match state {
                        crate::apps::manager::AppState::Inactive => AppStateOutput::Inactive,
                        crate::apps::manager::AppState::Switching { from, to } => {
                            AppStateOutput::Switching(
                                AppStateSwitchingOutput::new().with_from(from).with_to(to),
                            )
                        }
                        crate::apps::manager::AppState::Active { generation } => {
                            AppStateOutput::Active(AppStateActiveOutput::new(generation))
                        }
                        crate::apps::manager::AppState::Error {
                            generation,
                            message,
                        } => AppStateOutput::Error(AppStateErrorOutput::new(generation, message)),
                    };
                    let gen_entries: Vec<_> = generations
                        .iter()
                        .map(|gen| {
                            GenerationInfoOutput::new(
                                gen.meta.number,
                                gen.meta.created_at.clone(),
                                gen.complete,
                                Some(gen.meta.number) == current,
                            )
                            .with_last_activated(gen.meta.last_activated.clone())
                        })
                        .collect();
                    let output = AppInfoOutput::new(app.clone(), status, state_output, gen_entries);
                    rugix_cli::json::print_json(&output, false)
                        .whatever("unable to write app info to stdout")?;
                }
                AppsCommand::Activate { app, generation } => {
                    let gen = match generation {
                        Some(n) => *n,
                        None => {
                            let Some(n) = manager
                                .last_activated_generation(app)
                                .whatever("unable to find last activated generation")?
                            else {
                                bail!("no previously activated generation found for {app}");
                            };
                            n
                        }
                    };
                    manager
                        .activate_generation(app, gen)
                        .whatever("unable to activate generation")?;
                }
                AppsCommand::Deactivate { app } => {
                    manager
                        .deactivate(app)
                        .whatever("unable to deactivate app")?;
                }
                AppsCommand::Rollback { app } => {
                    manager.rollback(app).whatever("unable to rollback app")?;
                }
                AppsCommand::Remove { app } => {
                    manager.remove_app(app).whatever("unable to remove app")?;
                }
                AppsCommand::Generations { app } => {
                    use crate::config::output::GenerationInfoOutput;
                    let generations = manager
                        .list_generations(app)
                        .whatever("unable to list generations")?;
                    let current = manager.current_generation(app);
                    let entries: Vec<_> = generations
                        .iter()
                        .map(|gen| {
                            GenerationInfoOutput::new(
                                gen.meta.number,
                                gen.meta.created_at.clone(),
                                gen.complete,
                                Some(gen.meta.number) == current,
                            )
                            .with_last_activated(gen.meta.last_activated.clone())
                        })
                        .collect();
                    rugix_cli::json::print_json(&entries, false)
                        .whatever("unable to write generations to stdout")?;
                }
                AppsCommand::Gc { app, keep } => {
                    use crate::config::output::{AppGcAppOutput, AppGcOutput};
                    let app_names = match app {
                        Some(name) => vec![name.clone()],
                        None => manager.list_apps().whatever("unable to list apps")?,
                    };
                    let mut results = indexmap::IndexMap::new();
                    for name in &app_names {
                        let removed = manager
                            .gc(name, *keep)
                            .whatever("unable to garbage collect")?;
                        results.insert(name.clone(), AppGcAppOutput::new(removed));
                    }
                    rugix_cli::json::print_json(&AppGcOutput::new(results), false)
                        .whatever("unable to write gc output to stdout")?;
                }
                AppsCommand::Recover => {
                    manager.recover_all().whatever("recovery failed")?;
                }
                AppsCommand::CreateIndex {
                    app,
                    chunker,
                    hash_algorithm,
                    path,
                    generation,
                } => {
                    let gen_number = match generation {
                        Some(n) => *n,
                        None => manager
                            .current_generation(app)
                            .ok_or_else(|| whatever!("no active generation for app {app}"))?,
                    };
                    let gen_dir = manager.generation_dir(app, gen_number);
                    let paths: Vec<String> = match path {
                        Some(p) => vec![p.clone()],
                        None => {
                            let states =
                                crate::apps::manager::AppManager::load_payload_states(&gen_dir);
                            states.into_keys().collect()
                        }
                    };
                    for payload_path in &paths {
                        let data_file = gen_dir.join(payload_path);
                        if !data_file.exists() {
                            bail!(
                                "file {payload_path} not found in generation {gen_number} of app {app}"
                            );
                        }
                        info!(app = %app, path = %payload_path, "creating block index");
                        payload_db::add_app_file_index(
                            &gen_dir,
                            payload_path,
                            &data_file,
                            chunker,
                            hash_algorithm,
                        )?;
                    }
                }
                AppsCommand::Systemd(systemd_cmd) => match systemd_cmd {
                    AppsSystemdCommand::SyncUnits => {
                        crate::apps::generator::sync_units()
                            .whatever("failed to sync app units")?;
                    }
                },
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub enum ImageHash {
    Sha256(Vec<u8>),
}

pub enum MaybeStreamHasher<R> {
    NoHash {
        reader: R,
    },
    Sha256 {
        hasher: StreamHasher<R, sha2::Sha256>,
        expected: Vec<u8>,
    },
}

impl<R> MaybeStreamHasher<R> {
    pub fn verify(self) -> SystemResult<()> {
        match self {
            MaybeStreamHasher::NoHash { .. } => Ok(()),
            MaybeStreamHasher::Sha256 { hasher, expected } => {
                let found = hasher.finalize();
                if expected.as_slice() != found.as_slice() {
                    return Err(whatever(indoc::formatdoc! {
                        r#"
                            **Image Hash Mismatch:**
                            Expected: {}
                            Found: {}
                        "#,
                        hex::encode(expected),
                        hex::encode(found)
                    }));
                }
                Ok(())
            }
        }
    }
}

impl<R: Read> Read for MaybeStreamHasher<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            MaybeStreamHasher::NoHash { reader } => reader.read(buf),
            MaybeStreamHasher::Sha256 { hasher, .. } => hasher.read(buf),
        }
    }
}

fn install_app_bundle(
    config: &Config,
    app_manager: &crate::apps::manager::AppManager,
    bundle: &str,
    insecure_skip_bundle_verification: bool,
) -> SystemResult<()> {
    let file = File::open(bundle).whatever("unable to open app bundle")?;
    let bundle_source = ReaderSource::<_, SkipSeek>::from_unbuffered(file);
    let mut bundle_reader = rugix_bundle::reader::BundleReader::start(bundle_source, None)
        .whatever("unable to read app bundle")?;

    let root_cert = config.signatures.as_ref().and_then(|c| {
        if c.roots.len() > 1 {
            warn!("multiple root certificates in config, using only the first")
        };
        c.roots.first().map(|p| Path::new(p))
    });

    let bundle_verified = verify_bundle_signature(root_cert, &bundle_reader)?;
    if !bundle_verified && !insecure_skip_bundle_verification {
        bail!("bundle verification failed, refusing to install app bundle");
    }

    let mut app_generations: std::collections::HashMap<String, (u64, PathBuf)> =
        std::collections::HashMap::new();
    // Accumulated payload hashes per app, keyed by (app_name, path).
    let mut payload_states: std::collections::HashMap<
        String,
        std::collections::HashMap<String, payload_db::PayloadState>,
    > = std::collections::HashMap::new();

    let mut progress = |_source: &_| {};

    // Phase 1: extract all app payloads into generation directories.
    while let Some(payload) = bundle_reader
        .next_payload()
        .whatever("unable to read payload")?
    {
        let payload_entry = payload.entry();
        if let Some(type_app_file) = &payload_entry.type_app_file {
            let app_name = type_app_file.app.clone();
            let payload_path = type_app_file.path.clone();
            let delta_encoding = payload_entry.delta_encoding.clone();
            let (_, gen_dir) = app_generations.entry(app_name.clone()).or_insert_with(|| {
                app_manager
                    .create_generation(&app_name)
                    .expect("unable to create app generation")
            });
            let gen_dir = gen_dir.clone();
            let file_path = gen_dir.join(&payload_path);
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).whatever("unable to create parent directory")?;
            }

            // Set up block provider for block-encoded payloads.
            let mut block_provider = None;
            if let Some(block_encoding) = &payload.header().block_encoding {
                let mut provider = BlockProvider::new(
                    block_encoding.chunker.clone(),
                    block_encoding.hash_algorithm,
                );
                // Add block indices from existing generations of the same app.
                for gen in app_manager.list_generations(&app_name).unwrap_or_default() {
                    if !gen.complete {
                        continue;
                    }
                    let old_gen_dir = app_manager.generation_dir(&app_name, gen.meta.number);
                    let indices = payload_db::get_app_file_indices(&old_gen_dir, &payload_path)
                        .unwrap_or_default();
                    if !indices.is_empty() {
                        let data_file = old_gen_dir.join(&payload_path);
                        if data_file.exists() {
                            if let Err(e) = provider.add_indices(&indices, data_file) {
                                warn!("failed to load app-file block indices: {e:?}");
                            }
                        }
                    }
                }
                block_provider = Some(provider);
            }

            let decoded_payload_info = if let Some(delta_encoding) = delta_encoding {
                info!(
                    app = app_name,
                    path = payload_path,
                    "installing delta app file payload {}",
                    payload.idx()
                );
                if delta_encoding.inputs.len() != 1 {
                    bail!("unsupported number of delta encoding inputs");
                }
                let input = &delta_encoding.inputs[0];
                // Find the delta source in existing generations.
                let mut source_path = None;
                'generations: for gen in app_manager.list_generations(&app_name).unwrap_or_default()
                {
                    if !gen.complete {
                        continue;
                    }
                    let old_gen_dir = app_manager.generation_dir(&app_name, gen.meta.number);
                    let old_states =
                        crate::apps::manager::AppManager::load_payload_states(&old_gen_dir);
                    let Some(old_state) = old_states.get(&payload_path) else {
                        continue;
                    };
                    for input_hash in &input.hashes {
                        if let Some(stored_hash) = old_state.hashes.get(&input_hash.algorithm()) {
                            if stored_hash == input_hash {
                                let candidate = old_gen_dir.join(&payload_path);
                                if candidate.exists() {
                                    source_path = Some(candidate);
                                    break 'generations;
                                }
                            }
                        }
                    }
                }
                let Some(source_path) = source_path else {
                    bail!("no suitable delta source found for app-file {payload_path}");
                };
                match delta_encoding.format {
                    rugix_bundle::manifest::DeltaEncodingFormat::Xdelta => { /* ok */ }
                }
                let target = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .read(true)
                    .write(true)
                    .open(&file_path)
                    .whatever("unable to open app file target")?;
                let mut target_writer =
                    HashWriter::new(delta_encoding.original_hash.algorithm(), target);
                let (mut patch_reader, patch_writer) = buffered_pipe(8192);
                let (decode_result, xdelta_result) = std::thread::scope(|scope| {
                    let target_writer = &mut target_writer;
                    let handle = scope.spawn(move || {
                        xdelta_decompress(&source_path, &mut patch_reader, target_writer)
                    });
                    let decode_result = payload.decode_into(
                        BufferedPipeTarget {
                            writer: patch_writer,
                        },
                        block_provider
                            .as_ref()
                            .map(|p| p as &dyn StoredBlockProvider),
                        &mut progress,
                    );
                    (decode_result, handle.join().unwrap())
                });
                decode_result.whatever("unable to decode delta app payload")?;
                xdelta_result.whatever("unable to decompress delta app payload")?;
                let (target_hash, target_size) = target_writer.finalize();
                if target_hash != delta_encoding.original_hash {
                    bail!("decoded app file data does not match hash");
                }
                DecodedPayloadInfo {
                    hash: target_hash,
                    size: target_size.into(),
                }
            } else {
                info!(
                    app = app_name,
                    path = payload_path,
                    "extracting app file payload {}",
                    payload.idx()
                );
                let target = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .read(true)
                    .write(true)
                    .open(&file_path)
                    .whatever("unable to open app file target")?;
                payload
                    .decode_into(
                        target,
                        block_provider
                            .as_ref()
                            .map(|p| p as &dyn StoredBlockProvider),
                        &mut progress,
                    )
                    .whatever("unable to decode app payload")?
            };

            // Save payload hash for this app-file.
            payload_states.entry(app_name.clone()).or_default().insert(
                payload_path.clone(),
                payload_db::PayloadState {
                    hashes: [(
                        decoded_payload_info.hash.algorithm(),
                        decoded_payload_info.hash,
                    )]
                    .into_iter()
                    .collect(),
                    size: Some(decoded_payload_info.size),
                    updated_at: Some(jiff::Timestamp::now()),
                },
            );

            continue;
        }
        if let Some(type_app_archive) = &payload_entry.type_app_archive {
            let (_, gen_dir) = app_generations
                .entry(type_app_archive.app.clone())
                .or_insert_with(|| {
                    app_manager
                        .create_generation(&type_app_archive.app)
                        .expect("unable to create app generation")
                });
            info!(
                app = type_app_archive.app,
                "extracting app archive payload {}",
                payload.idx()
            );
            let tmp_tar = tempfile::NamedTempFile::new()
                .whatever("unable to create temporary file for archive")?;
            let tmp_file = tmp_tar
                .as_file()
                .try_clone()
                .whatever("unable to clone temp file handle")?;
            payload
                .decode_into(tmp_file, None, &mut progress)
                .whatever("unable to decode app archive payload")?;
            // Extract the tar archive into the generation directory.
            let tar_file = std::fs::File::open(tmp_tar.path())
                .whatever("unable to reopen archive for extraction")?;
            let mut archive = tar::Archive::new(tar_file);
            archive
                .unpack(gen_dir)
                .whatever("unable to extract app archive")?;
            continue;
        }
        payload.skip().whatever("unable to skip payload")?;
    }

    if app_generations.is_empty() {
        warn!("bundle contained no app payloads");
        return Ok(());
    }

    // Phase 2: save payload states, finalize, and activate.
    for (app_name, (gen_number, gen_dir)) in &app_generations {
        // Persist payload hashes for this generation.
        if let Some(states) = payload_states.get(app_name) {
            if let Err(e) = crate::apps::manager::AppManager::save_payload_states(gen_dir, states) {
                warn!(app = %app_name, "failed to save payload states: {e:?}");
            }
        }
        info!(app = %app_name, generation = gen_number, "finalizing app generation");
        app_manager
            .write_generation_metadata(
                gen_dir,
                &crate::apps::manager::AppGeneration {
                    number: *gen_number,
                    created_at: jiff::Timestamp::now().to_string(),
                    last_activated: None,
                },
            )
            .whatever("unable to write generation metadata")?;
        crate::apps::manager::AppManager::mark_complete(gen_dir)
            .whatever("unable to mark generation as complete")?;
        app_manager
            .activate_generation(app_name, *gen_number)
            .whatever("unable to activate app generation")?;
    }

    Ok(())
}

fn install_update_stream(
    system: &System,
    config: &Config,
    bundle: &String,
    boot_group: Option<&(BootGroupIdx, &BootGroup)>,
    bundle_hash: &Option<HashDigest>,
    root_cert: Option<&Path>,
    insecure_skip_bundle_verification: bool,
    disable_range_queries: bool,
    retry_config: RetryConfig,
) -> SystemResult<UpdateRebootType> {
    if bundle.starts_with("http") {
        let mut has_indices = false;
        for (_, slot) in system.slots().iter() {
            has_indices |= payload_db::get_stored_indices(slot.name())
                .map(|indices| !indices.is_empty())
                .unwrap_or_default();
            if has_indices {
                break;
            }
        }

        let mut bundle_source =
            HttpSource::new(bundle, !disable_range_queries && has_indices, retry_config)?;
        let should_reboot = install_update_bundle(
            system,
            config,
            &mut bundle_source,
            boot_group,
            bundle_hash,
            root_cert,
            insecure_skip_bundle_verification,
        )?;
        let stats = bundle_source.get_download_stats();
        info!(
            "downloaded {:.1}% ({}/{}) of the full bundle",
            stats.download_ratio() * 100.0,
            stats.bytes_read,
            stats.total_bytes(),
        );
        return Ok(should_reboot);
    }
    if bundle == "-" {
        let bundle_source = ReaderSource::<_, SkipRead>::from_unbuffered(io::stdin());
        install_update_bundle(
            system,
            config,
            bundle_source,
            boot_group,
            bundle_hash,
            root_cert,
            insecure_skip_bundle_verification,
        )
    } else {
        let file = File::open(bundle).whatever("error opening image")?;
        let bundle_source = ReaderSource::<_, SkipSeek>::from_unbuffered(file);
        install_update_bundle(
            system,
            config,
            bundle_source,
            boot_group,
            bundle_hash,
            root_cert,
            insecure_skip_bundle_verification,
        )
    }
}

pub struct UpdateState {
    bytes_read: u64,
    bytes_total: u64,
}

pub struct UpdateStatus {
    state: Mutex<UpdateState>,
}

impl StatusSegment for UpdateStatus {
    fn draw(&self, ctx: &mut rugix_cli::DrawCtx) {
        let state = self.state.lock().unwrap();
        if state.bytes_total > 0 {
            ProgressBar::new(state.bytes_read, state.bytes_total).draw(ctx);
        } else {
            ProgressSpinner::new().draw(ctx);
        }
    }
}

fn verify_bundle_signature<S: BundleSource>(
    root_cert: Option<&Path>,
    bundle_reader: &BundleReader<S>,
) -> SystemResult<bool> {
    let Some(root_cert) = root_cert else {
        return Ok(false);
    };
    let Some(signatures) = bundle_reader.signatures() else {
        warn!("root certificate configured but no signatures found");
        return Ok(false);
    };
    let cert_pem = std::fs::read(root_cert).whatever("unable to read root certificate")?;
    let verifier =
        rugix_pki::CmsVerifier::new(&cert_pem).whatever("unable to create CMS verifier")?;
    info!("checking bundle signatures");
    for signature in signatures.cms_signatures.iter() {
        let result = match verifier.verify(&signature.raw) {
            Ok(result) => result,
            Err(error) => {
                info!("signature verification failed: {error}");
                continue;
            }
        };
        let signed_metadata = decode_slice::<format::SignedMetadata>(&result.content)
            .whatever("unable to decode signed metadata")?;
        if signed_metadata.header_hash
            == bundle_reader.header_hash(signed_metadata.header_hash.algorithm())
        {
            info!("found valid signature");
            return Ok(true);
        }
    }
    Ok(false)
}

fn install_update_bundle<R: BundleSource>(
    system: &System,
    config: &Config,
    bundle_source: R,
    boot_group: Option<&(BootGroupIdx, &BootGroup)>,
    bundle_hash: &Option<HashDigest>,
    root_cert: Option<&Path>,
    insecure_skip_bundle_verification: bool,
) -> SystemResult<UpdateRebootType> {
    let mut bundle_reader =
        rugix_bundle::reader::BundleReader::start(bundle_source, bundle_hash.clone())
            .whatever("unable to read bundle")?;

    let root_cert = root_cert.or_else(|| {
        config.signatures.as_ref().and_then(|c| {
            if c.roots.len() > 1 {
                warn!("multiple root certificates in config, using only the first")
            };
            c.roots.get(0).map(|p| Path::new(p))
        })
    });

    // If a bundle hash has been specified, then the bundle will be verified against that hash
    // by the reader.
    let bundle_verified =
        bundle_hash.is_some() || verify_bundle_signature(root_cert, &bundle_reader)?;

    if !bundle_verified && !insecure_skip_bundle_verification {
        bail!("bundle verification failed, refusing to install update");
    }

    if !bundle_reader.header().is_incremental {
        let Some((entry_idx, _)) = boot_group else {
            bail!("full system updates require the specification of a boot group");
        };
        system
            .boot_flow()
            .pre_install(system, *entry_idx)
            .whatever("error executing pre-install step")?;
    }

    let update_status = rugix_cli::add_status(UpdateStatus {
        state: Mutex::new(UpdateState {
            bytes_read: 0,
            bytes_total: 0,
        }),
    });

    let mut progress = {
        let hooks = HooksLoader::default()
            .load_hooks("update-install")
            .whatever("unable to load `update-install` hooks")?;

        let mut last_progress = 0.0;
        move |source: &R| {
            let Some(bytes_total) = source.bytes_total() else {
                return;
            };
            let Some(bytes_read) = source.bytes_read() else {
                return;
            };
            let current_progress = (bytes_read.raw as f64) / (bytes_total.raw as f64) * 100.0;
            {
                let mut update_state = update_status.state.lock().unwrap();
                update_state.bytes_read = bytes_read.raw;
                update_state.bytes_total = bytes_total.raw;
            }
            if current_progress - last_progress > 0.9 {
                let hook_vars = vars! {
                    RUGIX_UPDATE_PROGRESS = format!("{current_progress:.2}")
                };
                if let Err(error) = hooks.run_hooks(
                    "progress",
                    hook_vars.clone(),
                    &RunOptions::default().with_silent(true),
                ) {
                    warn!("error running 'update-install/progress' hooks: {error:?}");
                }
                last_progress = current_progress;
            }
            if current_progress - last_progress > 0.4 && rugix_cli::stdout_is_piped() {
                let mut stdout = std::io::stdout();
                stdout
                    .write_all(
                        &serde_json::to_vec(&Event::UpdateProgress(UpdateProgressEvent {
                            progress: current_progress,
                        }))
                        .unwrap(),
                    )
                    .ok();
                stdout.write_all(b"\n").ok();
            }
        }
    };

    while let Some(payload) = bundle_reader
        .next_payload()
        .whatever("unable to read payload")?
    {
        let payload_entry = payload.entry();
        if let Some(slot_type) = &payload_entry.type_slot {
            let slot = boot_group
                .and_then(|(_, entry)| entry.get_slot(&slot_type.slot))
                .or_else(|| system.slots().find_by_name(&slot_type.slot).map(|e| e.0));
            if let Some(slot) = slot {
                let slot = &system.slots()[slot];
                info!(
                    "installing bundle payload {} to slot {}",
                    payload.idx(),
                    slot.name()
                );
                payload_db::erase(slot.name())?;
                let mut block_provider = None;
                if let Some(block_encoding) = &payload.header().block_encoding {
                    let mut provider = BlockProvider::new(
                        block_encoding.chunker.clone(),
                        block_encoding.hash_algorithm,
                    );
                    for (_, slot) in system.slots().iter() {
                        // Since we erased all the indices of the target slot, it
                        // is fine to also add the target slot here.
                        match slot.kind() {
                            SlotKind::Block(block_slot) => {
                                provider.add_slot(
                                    slot.name(),
                                    block_slot.device().path().to_path_buf(),
                                )?;
                            }
                            SlotKind::File { path } => {
                                provider.add_slot(slot.name(), path.to_path_buf())?;
                            }
                            SlotKind::Custom { .. } => { /* nothing to do */ }
                        }
                    }
                    block_provider = Some(provider);
                }
                let decoded_payload_info = if let Some(delta_encoding) =
                    &payload_entry.delta_encoding
                {
                    let delta_encoding = delta_encoding.clone();
                    if delta_encoding.inputs.len() != 1 {
                        bail!("unsupported number of delta encoding inputs");
                    }
                    let input = &delta_encoding.inputs[0];
                    let mut source = None;
                    'slots: for (_, delta_slot) in system.slots().iter() {
                        let Ok(Some(slot_state)) = payload_db::get_stored_state(delta_slot.name())
                        else {
                            continue;
                        };
                        for input_hash in &input.hashes {
                            let Some(slot_hash) = slot_state.hashes.get(&input_hash.algorithm())
                            else {
                                trace!(slot_name = delta_slot.name(), algorithm = ?input_hash.algorithm(), "no hash found");
                                continue;
                            };
                            if slot_hash == input_hash {
                                // We found the slot to use as a source.
                                source = Some(delta_slot);
                                trace!(slot_name = delta_slot.name(), "delta source found");
                                break 'slots;
                            } else {
                                trace!(slot_name = delta_slot.name(), %slot_hash, %input_hash, "hash does not match");
                            }
                        }
                    }
                    let Some(source) = source else {
                        bail!("no slot suitable delta source found");
                    };
                    // This is here so that we get an error when introducing additional formats.
                    match delta_encoding.format {
                        rugix_bundle::manifest::DeltaEncodingFormat::Xdelta => { /* do nothing */ }
                    }
                    let source = match source.kind() {
                        SlotKind::Block(block_slot) => block_slot.device().path().to_owned(),
                        SlotKind::File { path } => path.to_owned(),
                        SlotKind::Custom { .. } => {
                            bail!("source slot must not be a custom slot");
                        }
                    };
                    let target = match slot.kind() {
                        SlotKind::Block(block_slot) => std::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(block_slot.device())
                            .whatever("unable to open payload target")?,
                        SlotKind::File { path } => std::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(true)
                            .open(path)
                            .whatever("unable to open payload target")?,
                        SlotKind::Custom { .. } => {
                            bail!("custom slots do not support delta updates yet")
                        }
                    };
                    let mut target_writer =
                        HashWriter::new(delta_encoding.original_hash.algorithm(), target);
                    let (mut patch_reader, patch_writer) = buffered_pipe(8192);

                    let (decode_result, xdelta_result) = std::thread::scope(|scope| {
                        let target_writer = &mut target_writer;
                        // We must move the `patch_reader` here as we need it to be dropped when
                        // the decompression fails. Otherwise, we get a deadlock when waiting for
                        // the payload decoding in the following.
                        let handle = scope.spawn(move || {
                            trace!("starting xdelta");
                            let result =
                                xdelta_decompress(&source, &mut patch_reader, target_writer);
                            trace!(?result, "xdelta terminated");
                            result
                        });
                        let decode_result = payload.decode_into(
                            BufferedPipeTarget {
                                writer: patch_writer,
                            },
                            block_provider
                                .as_ref()
                                .map(|p| p as &dyn StoredBlockProvider),
                            &mut progress,
                        );
                        trace!("finished decoding payload into pipe");
                        (decode_result, handle.join().unwrap())
                    });
                    decode_result.whatever("unable to decode payload")?;
                    xdelta_result.whatever("unable to decode delta update")?;
                    let (target_hash, target_size) = target_writer.finalize();
                    if target_hash != delta_encoding.original_hash {
                        bail!("decoded slot data does not match hash");
                    }
                    DecodedPayloadInfo {
                        hash: target_hash,
                        size: target_size.into(),
                    }
                } else {
                    match slot.kind() {
                        SlotKind::Block(block_slot) => {
                            let target = std::fs::OpenOptions::new()
                                .read(true)
                                .write(true)
                                .open(block_slot.device())
                                .whatever("unable to open payload target")?;
                            payload
                                .decode_into(
                                    target,
                                    block_provider
                                        .as_ref()
                                        .map(|p| p as &dyn StoredBlockProvider),
                                    &mut progress,
                                )
                                .whatever("unable to decode payload")?
                        }
                        SlotKind::File { path } => {
                            let target = std::fs::OpenOptions::new()
                                .read(true)
                                .write(true)
                                .create(true)
                                .truncate(true)
                                .open(path)
                                .whatever("unable to open payload target")?;
                            payload
                                .decode_into(
                                    target,
                                    block_provider
                                        .as_ref()
                                        .map(|p| p as &dyn StoredBlockProvider),
                                    &mut progress,
                                )
                                .whatever("unable to decode payload")?
                        }
                        SlotKind::Custom { handler } => {
                            let target = CustomTarget::new(handler.iter().map(|arg| arg.as_str()))?;
                            payload
                                .decode_into(
                                    target,
                                    block_provider
                                        .as_ref()
                                        .map(|p| p as &dyn StoredBlockProvider),
                                    &mut progress,
                                )
                                .whatever("unable to decode payload")?
                        }
                    }
                };
                if let Err(error) = payload_db::save_slot_state(
                    slot.name(),
                    // Only save the hashes and size if the slot is immutable.
                    &SlotState {
                        hashes: if slot.is_immutable() {
                            [(
                                decoded_payload_info.hash.algorithm(),
                                decoded_payload_info.hash,
                            )]
                            .into_iter()
                            .collect()
                        } else {
                            Default::default()
                        },
                        size: if slot.is_immutable() {
                            Some(decoded_payload_info.size)
                        } else {
                            None
                        },
                        updated_at: Some(jiff::Timestamp::now()),
                    },
                ) {
                    error!("unable to save slot state: {error:?}");
                }
                continue;
            } else {
                error!(
                    "slot {:?} for bundle payload {} not found",
                    slot_type.slot,
                    payload.idx()
                );
            }
        } else if let Some(type_execute) = &payload_entry.type_execute {
            eprintln!("executing update payload {}", payload.idx(),);
            let target = CustomTarget::new(type_execute.handler.iter().map(|arg| arg.as_str()))?;
            payload
                .decode_into(target, None, &mut progress)
                .whatever("unable to decode payload")?;
            continue;
        }
        payload.skip().whatever("unable to skip payload")?;
    }

    if !bundle_reader.header().is_incremental {
        system
            .boot_flow()
            .post_install(system, boot_group.unwrap().0)
            .whatever("error executing post-install step")?;
        Ok(UpdateRebootType::Yes)
    } else {
        Ok(UpdateRebootType::No)
    }
}

#[derive(Debug)]
pub struct HashWriter<W> {
    writer: W,
    hasher: Hasher,
    size: u64,
}

impl<W> HashWriter<W> {
    pub fn new(algorithm: HashAlgorithm, writer: W) -> Self {
        Self {
            writer,
            hasher: algorithm.hasher(),
            size: 0,
        }
    }

    pub fn finalize(self) -> (HashDigest, u64) {
        (self.hasher.finalize(), self.size)
    }
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.size += buf.len() as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[derive(Debug)]
pub struct BufferedPipeTarget {
    writer: PipeWriter,
}

impl PayloadTarget for BufferedPipeTarget {
    fn write(&mut self, bytes: &[u8]) -> rugix_bundle::BundleResult<()> {
        self.writer.write_all(bytes).whatever("write failed")
    }

    fn finalize(mut self) -> rugix_bundle::BundleResult<()> {
        self.writer.flush().whatever("flush failed")
    }
}

#[derive(Debug)]
pub struct CustomTarget {
    child: Child,
}

impl CustomTarget {
    pub fn new<'arg>(mut command: impl Iterator<Item = &'arg str>) -> SystemResult<Self> {
        let Some(prog) = command.next() else {
            bail!("custom update handler cannot be an empty sequence");
        };
        let child = std::process::Command::new(prog)
            .args(command)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .whatever("unable to spawn custom update handler")?;
        Ok(Self { child })
    }
}

impl PayloadTarget for CustomTarget {
    fn write(&mut self, bytes: &[u8]) -> rugix_bundle::BundleResult<()> {
        self.child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(bytes)
            .whatever("unable to write payload to custom handler")
    }

    fn finalize(mut self) -> rugix_bundle::BundleResult<()> {
        info!("waiting on custom update handler to finalize");
        // Flush all bytes and close stdin.
        drop(self.child.stdin.take().unwrap());
        let status = self
            .child
            .wait()
            .whatever("error waiting for update handler")?;
        if !status.success() {
            bail!(
                "error running custom update handler, code {:?}",
                status.code()
            )
        }
        Ok(())
    }
}

#[derive(Debug, Clone, ValueEnum)]
pub enum Boolean {
    True,
    False,
}

#[derive(Debug, Parser)]
#[clap(author, version = rugix_version::RUGIX_GIT_VERSION, about)]
pub struct Args {
    /// The command.
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Debug, Parser)]
pub enum Command {
    /// Manage the persistent state of the system.
    #[clap(subcommand)]
    State(StateCommand),
    /// Install and inspect over-the-air updates.
    #[clap(subcommand)]
    Update(UpdateCommand),
    /// Manage the system.
    #[clap(subcommand)]
    System(SystemCommand),
    /// Manage the update slots of the system.
    #[clap(subcommand)]
    Slots(SlotsCommand),
    /// Control the boot flow of the system.
    #[clap(subcommand)]
    Boot(BootCommand),
    /// Utility commands useful for scripting.
    #[clap(subcommand)]
    Utils(UtilsCommand),
    /// Manage applications.
    #[clap(subcommand)]
    Apps(AppsCommand),
    /// Unstable experimental commands.
    #[clap(subcommand)]
    Unstable(UnstableCommand),
}

#[derive(Debug, Parser)]
pub enum StateCommand {
    /// Perform a factory reset of the system.
    Reset {
        /// Backup the old state by creating a new state profile.
        #[clap(long)]
        backup: bool,
        /// Name of the backup state profile.
        #[clap(long)]
        backup_name: Option<String>,
    },
    /// Configure the root filesystem overlay.
    #[clap(subcommand)]
    Overlay(OverlayCommand),
}

#[derive(Debug, Parser)]
pub enum SlotsCommand {
    /// Verify the integrity of a slot.
    Verify { slot: String },
    /// Query the state of a slot.
    Inspect { slot: String },
    /// Add an index to a slot.
    CreateIndex {
        slot: String,
        chunker: ChunkerAlgorithm,
        hash_algorithm: HashAlgorithm,
    },
}

#[derive(Debug, Parser)]
pub enum OverlayCommand {
    /// Set the persistency of the overlay.
    ForcePersist { persist: Boolean },
}

#[derive(Debug, Parser)]
pub enum UpdateCommand {
    /// Install an update.
    Install {
        /// Path to the update bundle to install.
        bundle: String,
        /// Skip bundle verification (insecure, do not use in production).
        ///
        /// By default, either a valid signature is required or a bundle hash has to be
        /// specified with `--bundle-hash`. This flag allows the installation of update
        /// bundles without either of those.
        #[clap(long)]
        insecure_skip_bundle_verification: bool,
        /// Root certificate to use for signature verification.
        ///
        /// This overrides the configured default certificate.
        #[clap(long)]
        root_cert: Option<PathBuf>,
        /// Expected bundle hash.
        #[clap(long)]
        bundle_hash: Option<HashDigest>,
        /// Control how to reboot the system.
        #[clap(long)]
        reboot: Option<UpdateRebootType>,
        /// Do not delete the overlay of the target slot (if any).
        ///
        /// Only effective when using Rugix's state management mechanism.
        #[clap(long)]
        keep_overlay: bool,
        /// Boot group to install the update to.
        #[clap(long)]
        boot_group: Option<String>,
        /// Disable the use of range queries for HTTP sources.
        #[clap(long)]
        disable_range_queries: bool,
        /// Maximum number of retry attempts for transient HTTP errors.
        #[clap(long, default_value_t = 5)]
        http_max_retries: u32,
        /// Initial HTTP retry backoff duration in seconds.
        #[clap(long, default_value_t = 1)]
        http_retry_initial_backoff: u64,
        /// Maximum HTTP retry backoff duration in seconds.
        #[clap(long, default_value_t = 30)]
        http_retry_max_backoff: u64,
    },
}

#[derive(Debug, Clone, ValueEnum)]
pub enum UpdateRebootType {
    /// Reboot into the newly installed system.
    Yes,
    /// Do nothing.
    No,
    /// Just set the flags without rebooting.
    ///
    /// This will tell the bootloader integration to boot into the new system next without
    /// actually triggering a reboot.
    Set,
    /// Set the deferred spare reboot marker.
    ///
    /// Rugix will itself remember that an update has been installed. On the next boot,
    /// it will remove the marker and reboot into the new system. This allows the system
    /// to be shutoff after installing the update. On the next boot, Rugix will then try
    /// to boot into the new version.
    Deferred,
}

#[derive(Debug, Parser)]
pub enum SystemCommand {
    /// Print information about the system.
    Info {
        /// Output compact JSON instead of pretty-printed JSON.
        #[clap(long)]
        json: bool,
    },
    /// Make the active system the default.
    Commit,
    /// Reboot the system.
    Reboot {
        /// Reboot into the spare system.
        #[clap(long)]
        spare: bool,
    },
}

#[derive(Debug, Parser)]
pub enum UnstableCommand {
    /// Set deferred spare reboot flag.
    SetDeferredSpareReboot {
        value: Boolean,
    },
    PrintSystemInfo,
}

#[derive(Debug, Parser)]
pub enum BootCommand {
    /// Mark a boot group as good.
    MarkGood { group: Option<String> },
    /// Mark a boot group as bad.
    MarkBad { group: String },
}

#[derive(Debug, Parser)]
pub enum UtilsCommand {
    /// Determine the block device behind a path.
    FindBlockDevice { path: PathBuf },
    /// Check whether a path is a mount point.
    IsMountPoint { path: PathBuf },
    /// Resolve a partition relative to the main disk or some other block device.
    ResolvePartition {
        #[clap(long)]
        disk: Option<PathBuf>,
        partition: u32,
    },
}

#[derive(Debug, Parser)]
pub enum AppsCommand {
    /// Install apps from a bundle.
    Install {
        /// Path or URL of the app bundle.
        bundle: String,
        /// Skip bundle signature verification.
        #[clap(long)]
        insecure_skip_bundle_verification: bool,
    },
    /// List all installed apps.
    List,
    /// Show app info and status.
    Info {
        /// App name.
        app: String,
    },
    /// Activate a generation. If no generation is specified, activates the
    /// most recently activated generation (useful for re-activating after deactivation).
    Activate {
        /// App name.
        app: String,
        /// Generation number (defaults to the most recently activated generation).
        generation: Option<u64>,
    },
    /// Deactivate the current generation of an app.
    Deactivate {
        /// App name.
        app: String,
    },
    /// Rollback an app to the previous generation.
    Rollback {
        /// App name.
        app: String,
    },
    /// Remove an app entirely.
    Remove {
        /// App name.
        app: String,
    },
    /// List generations for an app.
    Generations {
        /// App name.
        app: String,
    },
    /// Garbage collect old generations. If no app is specified, runs for all apps.
    Gc {
        /// App name (if omitted, runs for all apps).
        app: Option<String>,
        /// Number of previously activated generations to keep (in addition to the
        /// currently active generation, which is always kept).
        #[clap(long, default_value_t = 1)]
        keep: usize,
    },
    /// Recover any interrupted app transitions.
    Recover,
    /// Create block indices for app-file payloads in a generation.
    ///
    /// If no path is specified, creates indices for all payload files
    /// recorded in the generation's payloads.json.
    CreateIndex {
        /// App name.
        app: String,
        /// Chunker algorithm.
        chunker: ChunkerAlgorithm,
        /// Hash algorithm.
        hash_algorithm: HashAlgorithm,
        /// Relative payload path within the generation directory.
        /// If omitted, creates indices for all known payload files.
        #[clap(long)]
        path: Option<String>,
        /// Generation number (defaults to the currently active generation).
        #[clap(long)]
        generation: Option<u64>,
    },
    /// Systemd integration.
    #[clap(subcommand)]
    Systemd(AppsSystemdCommand),
}

#[derive(Debug, Parser)]
pub enum AppsSystemdCommand {
    /// Sync app units into the systemd runtime directory.
    SyncUnits,
}
