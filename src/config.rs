use std::fs;
use std::path::Path;
use std::process::Command;

/// Generates /etc/fstab and /etc/hostname for the target installation
pub fn configure_system(
    target_mount: &Path,
    root_partition: &Path,
    efi_partition: &Path,
    hostname: &str,
) -> Result<(), String> {
    write_fstab(target_mount, root_partition, efi_partition)?;
    write_hostname(target_mount, hostname)?;
    Ok(())
}

fn write_fstab(target_mount: &Path, root_part: &Path, efi_part: &Path) -> Result<(), String> {
    let root_uuid = get_uuid(root_part)?;
    let efi_uuid = get_uuid(efi_part)?;

    let fstab_content = format!(
        "# /etc/fstab: static file system information.\n\
         # <file system>                           <mount point>  <type>  <options>       <dump>  <pass>\n\
         UUID={:<36}  /              ext4    defaults        0       1\n\
         UUID={:<36}  /boot/efi      vfat    defaults        0       2\n",
        root_uuid, efi_uuid
    );

    let etc_dir = target_mount.join("etc");
    fs::create_dir_all(&etc_dir)
        .map_err(|e| format!("Failed to create /etc directory: {}", e))?;

    let fstab_path = etc_dir.join("fstab");
    fs::write(&fstab_path, fstab_content)
        .map_err(|e| format!("Failed to write /etc/fstab: {}", e))?;

    Ok(())
}

fn write_hostname(target_mount: &Path, hostname: &str) -> Result<(), String> {
    let hostname_path = target_mount.join("etc/hostname");
    fs::write(&hostname_path, format!("{}\n", hostname.trim()))
        .map_err(|e| format!("Failed to write /etc/hostname: {}", e))?;
    Ok(())
}

/// Helper function to retrieve the UUID of a given partition using `blkid`
fn get_uuid(partition_path: &Path) -> Result<String, String> {
    let output = Command::new("blkid")
        .args(["-s", "UUID", "-o", "value", partition_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to execute blkid: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to retrieve UUID for {:?}: {}",
            partition_path,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uuid.is_empty() {
        return Err(format!("blkid returned empty UUID for {:?}", partition_path));
    }

    Ok(uuid)
}
