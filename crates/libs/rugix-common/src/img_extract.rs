//! Utilities for extracting partitions and filesystems from disk images.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use reportify::{bail, Report, ResultExt};
use tracing::info;
use xscript::{read_str, run, Run};

use crate::disk::{Partition, PartitionTable};
use crate::partitions::DiskError;
use crate::utils::units::NumBytes;

/// Filesystem type detected by probing the partition image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsType {
    /// FAT filesystem (FAT12, FAT16, FAT32).
    Fat,
    /// Linux ext filesystem (ext2, ext3, ext4).
    Ext,
    /// Unknown or unsupported filesystem type.
    Unknown(String),
}

impl FsType {
    /// Probe the filesystem type of an image file using `blkid`.
    ///
    /// This examines the actual filesystem signatures in the image rather than
    /// relying on partition type metadata.
    pub fn probe(image_path: &Path) -> Result<Self, Report<DiskError>> {
        let output = read_str!(["blkid", "-o", "value", "-s", "TYPE", image_path])
            .whatever("unable to probe filesystem type with blkid")?;
        let fs_type = output.trim();
        Ok(Self::from_blkid_type(fs_type))
    }

    /// Convert a blkid TYPE string to FsType.
    fn from_blkid_type(ty: &str) -> Self {
        match ty {
            "vfat" | "fat" | "fat12" | "fat16" | "fat32" | "msdos" => FsType::Fat,
            "ext2" | "ext3" | "ext4" => FsType::Ext,
            "" => FsType::Unknown("empty or unformatted".to_owned()),
            other => FsType::Unknown(other.to_owned()),
        }
    }

    /// Check if this is a known, extractable filesystem type.
    pub fn is_supported(&self) -> bool {
        matches!(self, FsType::Fat | FsType::Ext)
    }
}

/// Extract a partition from a disk image to a separate file.
///
/// This function reads the raw partition data from the disk image at the correct offset
/// and writes it to the destination file.
pub fn extract_partition(
    image_path: &Path,
    partition: &Partition,
    block_size: NumBytes,
    dst_path: &Path,
) -> Result<(), Report<DiskError>> {
    let start_bytes = partition.start.into_raw() * block_size.into_raw();
    let size_bytes = partition.size.into_raw() * block_size.into_raw();

    let mut src = File::open(image_path).whatever("unable to open disk image")?;
    let mut dst = File::create(dst_path).whatever("unable to create partition file")?;

    src.seek(SeekFrom::Start(start_bytes))
        .whatever("unable to seek in disk image")?;

    const CHUNK_SIZE: usize = 4 * 1024 * 1024;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut remaining = size_bytes as usize;

    while remaining > 0 {
        let to_read = remaining.min(CHUNK_SIZE);
        let bytes_read = src
            .read(&mut buffer[..to_read])
            .whatever("unable to read from disk image")?;
        if bytes_read == 0 {
            break;
        }
        dst.write_all(&buffer[..bytes_read])
            .whatever("unable to write to partition file")?;
        remaining -= bytes_read;
    }

    Ok(())
}

/// Extract filesystem contents from a partition image to a directory.
///
/// This function uses `debugfs` for ext filesystems and `mcopy` for FAT filesystems.
pub fn extract_filesystem(
    partition_image: &Path,
    dst_dir: &Path,
    fs_type: &FsType,
) -> Result<(), Report<DiskError>> {
    std::fs::create_dir_all(dst_dir).whatever("unable to create destination directory")?;

    let partition_image = partition_image
        .canonicalize()
        .whatever("unable to canonicalize partition image path")?;

    match fs_type {
        FsType::Ext => {
            run!(["debugfs", "-R", "rdump / .", &partition_image].with_cwd(dst_dir))
                .whatever("unable to extract ext filesystem with debugfs")?;
        }
        FsType::Fat => {
            run!(["mcopy", "-i", &partition_image, "-snop", "::", dst_dir])
                .whatever("unable to extract FAT filesystem with mcopy")?;
        }
        FsType::Unknown(ty) => {
            bail!("cannot extract filesystem: unsupported type {:?}", ty);
        }
    }

    Ok(())
}

/// Extract partitions from a disk image and extract their filesystem contents.
///
/// Returns the partition table for further inspection if needed.
pub fn extract_image_partitions(
    image_path: &Path,
    partitions_config: &[(u8, &Path)],
    temp_dir: &Path,
) -> Result<PartitionTable, Report<DiskError>> {
    let table = PartitionTable::read(image_path)?;

    for (part_num, dst_dir) in partitions_config {
        let partition = table
            .partitions
            .iter()
            .find(|p| p.number == *part_num)
            .ok_or_else(|| reportify::whatever!("partition {} not found in image", part_num))?;

        let part_image_path = temp_dir.join(format!("partition-{}.img", part_num));
        extract_partition(image_path, partition, table.block_size, &part_image_path)?;

        let fs_type = FsType::probe(&part_image_path)?;
        if !fs_type.is_supported() {
            // Clean up before bailing
            std::fs::remove_file(&part_image_path).ok();
            bail!(
                "partition {} has unsupported filesystem type: {:?}",
                part_num,
                fs_type
            );
        }

        extract_filesystem(&part_image_path, dst_dir, &fs_type)?;

        info!(
            "extracted partition {} ({:?}) to {:?}",
            part_num, fs_type, dst_dir
        );

        std::fs::remove_file(&part_image_path).ok();
    }

    Ok(table)
}
