use std::path::Path;
use std::process::Command;
use crate::config::FilesystemType;

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



pub fn format_root_partition(partition: &Path, fs_type: FilesystemType) -> Result<(), String> {
    let status = match fs_type {
        FilesystemType::Ext4 => {
            Command::new("mkfs.ext4")
                .args(["-F", partition.to_str().unwrap()])
                .status()
        }
        FilesystemType::Btrfs => {
            Command::new("mkfs.btrfs")
                .args(["-f", partition.to_str().unwrap()])
                .status()
        }
    };

    let status = status.map_err(|e| format!("Failed to format root partition: {}", e))?;
    if !status.success() {
        return Err("mkfs command failed".to_string());
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

