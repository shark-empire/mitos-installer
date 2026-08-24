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
