use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct PartitionLayout {
    pub efi_partition: PathBuf,
    pub root_partition: PathBuf,
}

pub fn partition_target_disk(disk_path: &Path) -> Result<PartitionLayout, String> {
    wipe_partition_table(disk_path)?;
    create_gpt_layout(disk_path)?;

    Ok(PartitionLayout {
        efi_partition: get_partition_path(disk_path, 1),
        root_partition: get_partition_path(disk_path, 2),
    })
}

fn wipe_partition_table(disk_path: &Path) -> Result<(), String> {
    // 1. Wipe filesystem signatures to prevent ghost filesystems
    Command::new("wipefs")
        .args(["-a", disk_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to run wipefs: {}", e))?;

    // 2. Zap the GPT/MBR tables entirely
    let status = Command::new("sgdisk")
        .args(["--zap-all", disk_path.to_str().unwrap()])
        .status()
        .map_err(|e| format!("Failed to execute sgdisk zap: {}", e))?;

    if !status.success() {
        return Err(format!("Failed to wipe partition table on {:?}", disk_path));
    }
    
    Ok(())
}

fn create_gpt_layout(disk_path: &Path) -> Result<(), String> {
    let disk_str = disk_path.to_str().unwrap();

    let status = Command::new("sgdisk")
        // Clear all partition data in memory before writing
        .arg("--clear")
        // Partition 1: EFI System Partition (512MB, type ef00)
        .args(["--new=1:0:+512M", "--typecode=1:ef00", "--change-name=1:MITOS_EFI"])
        // Partition 2: Root Filesystem (Remaining space, type 8300)
        .args(["--new=2:0:0", "--typecode=2:8300", "--change-name=2:MITOS_ROOT"])
        .arg(disk_str)
        .status()
        .map_err(|e| format!("Failed to execute sgdisk partitioning: {}", e))?;

    if !status.success() {
        return Err(format!("Failed to create GPT layout on {:?}", disk_path));
    }

    // Force the kernel to re-read the partition table immediately
    let _ = Command::new("partprobe").arg(disk_str).status();

    Ok(())
}

/// Automatically handles standard block naming (sda -> sda1) 
/// and NVMe/MMC block naming (nvme0n1 -> nvme0n1p1)
fn get_partition_path(disk: &Path, part_num: u8) -> PathBuf {
    let path_str = disk.to_string_lossy();
    let suffix = if path_str.contains("nvme") || path_str.contains("mmc") || path_str.contains("loop") {
        format!("p{}", part_num)
    } else {
        format!("{}", part_num)
    };
    
    PathBuf::from(format!("{}{}", path_str, suffix))
}
