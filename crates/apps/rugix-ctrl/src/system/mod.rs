use boot_flows::BootFlow;
use boot_groups::{BootGroup, BootGroupIdx, BootGroups};
use config::load_system_config;
use partitions::ConfigPartition;
use reportify::{bail, whatever, Report, ResultExt};
use root::{find_system_device, SystemRoot};
use slots::{SlotKind, SystemSlots};
use tracing::warn;

use rugix_common::disk::blkdev::BlockDevice;

use crate::config::system::{PartitionConfig, SystemConfig};

pub mod boot_flows;
pub mod boot_groups;
pub mod config;
pub mod partitions;
pub mod paths;
pub mod root;
pub mod slots;

reportify::new_whatever_type! {
    SystemError
}

pub type SystemResult<T> = Result<T, Report<SystemError>>;

pub struct System {
    pub config: SystemConfig,
    pub device: Option<BlockDevice>,
    pub root: Option<SystemRoot>,

    slots: SystemSlots,
    boot_entries: BootGroups,
    active_boot_entry: Option<BootGroupIdx>,
    boot_flow: Box<dyn BootFlow>,
    config_partition: Option<ConfigPartition>,
}

impl System {
    pub fn initialize() -> SystemResult<Self> {
        let system_config = load_system_config()?;
        let system_device = find_system_device();
        let system_root = system_device
            .as_ref()
            .and_then(SystemRoot::from_system_device);

        let config_partition = ConfigPartition::from_config(
            system_config
                .config_partition
                .as_ref()
                .unwrap_or(&PartitionConfig::new()),
        );
        let slots = SystemSlots::from_config(system_root.as_ref(), system_config.slots.as_ref())?;
        let boot_entries = BootGroups::from_config(&slots, system_config.boot_groups.as_ref())?;
        // Create boot flow before determining the active group so that the
        // boot flow can provide this information (e.g., via EFI variables).
        let boot_flow = boot_flows::from_config(
            system_config.boot_flow.as_ref(),
            config_partition.as_ref(),
            &boot_entries,
        )
        .whatever("unable to create boot flow from config")?;
        // Determine the active boot group. First, ask the boot flow — it may
        // know directly (e.g., systemd-boot reads LoaderEntrySelected).
        let mut active_boot_entry = boot_flow
            .get_active(&boot_entries)
            .whatever("unable to determine active boot group from boot flow")?;
        // Fall back to matching block devices against the system device.
        if active_boot_entry.is_none() {
            for (idx, entry) in boot_entries.iter() {
                for (_, slot) in entry.slots() {
                    if let SlotKind::Block(raw) = &slots[slot].kind() {
                        if Some(raw.device()) == system_device.as_ref() {
                            entry.mark_active();
                            break;
                        }
                    }
                }
                if entry.active() {
                    active_boot_entry = Some(idx);
                    break;
                }
            }
        }
        // Mark all slots in the active group as active.
        if let Some(active_idx) = active_boot_entry {
            let entry = &boot_entries[active_idx];
            entry.mark_active();
            for (_, slot) in entry.slots() {
                slots[slot].mark_active();
            }
        }
        if active_boot_entry.is_none() {
            warn!("unable to determine active boot group");
        }
        Ok(Self {
            config: system_config,
            device: system_device,
            root: system_root,
            slots,
            boot_entries,
            active_boot_entry,
            boot_flow,
            config_partition,
        })
    }

    pub fn root(&self) -> &Option<SystemRoot> {
        &self.root
    }

    pub fn config(&self) -> &SystemConfig {
        &self.config
    }

    pub fn slots(&self) -> &SystemSlots {
        &self.slots
    }

    pub fn boot_entries(&self) -> &BootGroups {
        &self.boot_entries
    }

    pub fn active_boot_entry(&self) -> Option<BootGroupIdx> {
        self.active_boot_entry
    }

    /// First entry that is not the default.
    pub fn spare_entry(&self) -> SystemResult<Option<(BootGroupIdx, &BootGroup)>> {
        let default = self
            .boot_flow
            .get_default(self)
            .whatever("unable to determine default boot group")?;
        Ok(self.boot_entries().iter().find(|(idx, _)| *idx != default))
    }

    pub fn needs_commit(&self) -> SystemResult<bool> {
        Ok(self.active_boot_entry
            != Some(
                self.boot_flow
                    .get_default(self)
                    .whatever("unable to determine default boot group")?,
            ))
    }

    pub fn boot_flow(&self) -> &dyn BootFlow {
        &*self.boot_flow
    }

    pub fn config_partition(&self) -> Option<&ConfigPartition> {
        self.config_partition.as_ref()
    }

    pub fn require_config_partition(&self) -> SystemResult<&ConfigPartition> {
        self.config_partition()
            .ok_or_else(|| whatever("config partition is required"))
    }

    pub fn commit(&self) -> SystemResult<()> {
        self.boot_flow
            .commit(self)
            .whatever("unable to commit to active boot group")
    }
}
