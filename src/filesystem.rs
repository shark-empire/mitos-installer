use std::path::Path;
use std::process::Command;

/// Formats the EFI system partition as FAT32 with label "BOOT"
pub fn format_efi_partition(partition_path: &Path) -> Result<(), String> {
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

/// Formats the root partition as EXT4 with label "MITOS_ROOT"
pub fn format_root_partition(partition_path: &Path) -> Result<(), String> {
    let output = Command::new("mkfs.ext4")
        .args(["-F", "-L", "MITOS_ROOT", partition_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to execute mkfs.ext4: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to format root partition {:?}: {}",
            partition_path,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

// Add this to filesystem.rs
pub fn create_btrfs_layout(root_partition: &Path) -> Result<(), String> {
    // 1. Format as Btrfs
    Command::new("mkfs.btrfs").arg("-f").arg(root_partition).status()
        .map_err(|e| e.to_string())?;

    // 2. Temporary mount to create subvolumes
    let temp_mount = Path::new("/mnt/mitos-temp");
    std::fs::create_dir_all(temp_mount).unwrap();
    Command::new("mount").arg(root_partition).arg(temp_mount).status().unwrap();

    // 3. Create standard subvolumes
    for sv in ["@", "@home", "@var", "@snapshots", "@log"] {
        let sv_path = temp_mount.join(sv);
        Command::new("btrfs")
            .args(["subvolume", "create", sv_path.to_str().unwrap()])
            .status()
            .map_err(|e| format!("Failed to create {}: {}", sv, e))?;
    }

    // 4. Unmount temp
    Command::new("umount").arg(temp_mount).status().unwrap();
    Ok(())
}

