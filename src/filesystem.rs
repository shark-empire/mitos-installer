use crate::config::FilesystemType;
use log::info;
use std::path::Path;
use std::process::Command;

/// Formats the EFI system partition as FAT32 with label "BOOT"
pub fn format_efi_partition(partition_path: &Path) -> Result<(), String> {
    info!("Formatting {:?} as FAT32 (ESP)...", partition_path);
    let output = Command::new("mkfs.vfat")
        .args(["-F", "32", "-n", "BOOT", partition_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to execute mkfs.vfat: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to format EFI partition {:?}: {}",
            partition_path,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

pub fn format_root_partition(partition: &Path, fs_type: FilesystemType) -> Result<(), String> {
    info!("Formatting {:?} as {:?}...", partition, fs_type);
    let output = match fs_type {
        FilesystemType::Ext4 => Command::new("mkfs.ext4")
            .args(["-F", "-L", "MITOS_ROOT", partition.to_str().unwrap()])
            .output(),
        FilesystemType::Btrfs => Command::new("mkfs.btrfs")
            .args(["-f", "-L", "MITOS_ROOT", partition.to_str().unwrap()])
            .output(),
    };

    let output = output.map_err(|e| format!("Failed to run mkfs on {:?}: {}", partition, e))?;
    if !output.status.success() {
        return Err(format!(
            "mkfs failed on {:?}: {}",
            partition,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Formats `root_partition` as Btrfs and creates the standard MITOS subvolume layout
/// (@, @home, @var, @snapshots, @log) via a short-lived temporary mount.
pub fn create_btrfs_layout(root_partition: &Path) -> Result<(), String> {
    // 1. Format as Btrfs
    let output = Command::new("mkfs.btrfs")
        .args(["-f", "-L", "MITOS_ROOT", root_partition.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to run mkfs.btrfs: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "mkfs.btrfs failed on {:?}: {}",
            root_partition,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // 2. Temporary mount to create subvolumes
    let temp_mount = Path::new("/mnt/mitos-temp");
    std::fs::create_dir_all(temp_mount).map_err(|e| {
        format!(
            "Failed to create temporary mount point {:?}: {}",
            temp_mount, e
        )
    })?;

    let mount_status = Command::new("mount")
        .arg(root_partition)
        .arg(temp_mount)
        .status()
        .map_err(|e| {
            format!(
                "Failed to mount {:?} for subvolume creation: {}",
                root_partition, e
            )
        })?;
    if !mount_status.success() {
        return Err(format!(
            "Failed to temporarily mount {:?} at {:?} to create subvolumes",
            root_partition, temp_mount
        ));
    }

    // 3. Create standard subvolumes. If any step fails, still attempt to unmount before
    // returning, so we don't leave a stray mount behind on error.
    let mut subvolume_result = Ok(());
    for sv in ["@", "@home", "@var", "@snapshots", "@log"] {
        let sv_path = temp_mount.join(sv);
        let status = Command::new("btrfs")
            .args(["subvolume", "create", sv_path.to_str().unwrap()])
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(_) => {
                subvolume_result = Err(format!("btrfs subvolume create failed for {}", sv));
                break;
            }
            Err(e) => {
                subvolume_result = Err(format!(
                    "Failed to run btrfs subvolume create for {}: {}",
                    sv, e
                ));
                break;
            }
        }
    }

    // 4. Unmount temp (best-effort; the outcome of subvolume creation is what we report)
    let _ = Command::new("umount").arg(temp_mount).status();

    subvolume_result
}

/// Formats `swap_partition` as Linux swap and activates it immediately, so subsequent
/// steps in the pipeline (which may be memory-hungry, e.g. package installs) benefit
/// from it too. Swap does not need to be deactivated before reboot, but it does need to
/// be deactivated before the disk can be safely re-partitioned during rollback - see
/// `recovery::trigger_emergency_cleanup`.
pub fn format_and_enable_swap(swap_partition: &Path) -> Result<(), String> {
    info!("Formatting {:?} as swap...", swap_partition);
    let mkswap_output = Command::new("mkswap")
        .args(["-L", "MITOS_SWAP", swap_partition.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to run mkswap on {:?}: {}", swap_partition, e))?;
    if !mkswap_output.status.success() {
        return Err(format!(
            "mkswap failed on {:?}: {}",
            swap_partition,
            String::from_utf8_lossy(&mkswap_output.stderr)
        ));
    }

    let swapon_status = Command::new("swapon")
        .arg(swap_partition)
        .status()
        .map_err(|e| format!("Failed to run swapon on {:?}: {}", swap_partition, e))?;
    if !swapon_status.success() {
        return Err(format!("swapon failed to activate {:?}", swap_partition));
    }

    Ok(())
}

/// Deactivates swap on `swap_partition`. Best-effort: used during rollback, where the
/// disk is about to be wiped regardless, so a failure here is logged but not fatal.
pub fn disable_swap(swap_partition: &Path) {
    let _ = Command::new("swapoff").arg(swap_partition).status();
}
