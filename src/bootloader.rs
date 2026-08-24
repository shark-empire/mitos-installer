use std::fs;
use std::path::Path;
use std::process::Command;

/// Installs Limine EFI binaries and writes the configuration file
pub fn install_limine(
    efi_mount: &Path,
    root_partition: &Path,
    kernel_name: &str, // Defaults to "bzImage" based on your build script
) -> Result<(), String> {
    let efi_boot_dir = efi_mount.join("EFI").join("BOOT");

    // 1. Create the EFI boot directory hierarchy
    fs::create_dir_all(&efi_boot_dir)
        .map_err(|e| format!("Failed to create EFI directory {:?}: {}", efi_boot_dir, e))?;

    // 2. Copy the Limine EFI binary
    // This assumes the installer environment has BOOTX64.EFI available at /usr/share/limine/BOOTX64.EFI
    let limine_src = Path::new("/usr/share/limine/BOOTX64.EFI");
    let limine_dest = efi_boot_dir.join("BOOTX64.EFI");
    
    if !limine_src.exists() {
        return Err("Limine EFI binary not found in the live environment at /usr/share/limine/BOOTX64.EFI".to_string());
    }

    fs::copy(limine_src, &limine_dest)
        .map_err(|e| format!("Failed to copy Limine EFI binary: {}", e))?;

    // 3. Extract the PARTUUID of the target root partition
    let partuuid = get_partuuid(root_partition)?;

    // 4. Generate limine.conf specifically for the Linux protocol
    let limine_conf_content = format!(
        "timeout: 3\n\
        default_entry: 1\n\
        \n\
        /MITOS Linux\n\
            protocol: linux\n\
            kernel_path: boot():/{}\n\
            cmdline: root=PARTUUID={} rw quiet splash\n",
        kernel_name, partuuid
    );

    let conf_dest = efi_boot_dir.join("limine.conf");
    fs::write(&conf_dest, limine_conf_content)
        .map_err(|e| format!("Failed to write limine.conf: {}", e))?;

    Ok(())
}

/// Helper function to retrieve the PARTUUID of a given partition using `blkid`
fn get_partuuid(partition_path: &Path) -> Result<String, String> {
    let output = Command::new("blkid")
        .args(["-s", "PARTUUID", "-o", "value", partition_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to execute blkid: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to retrieve PARTUUID for {:?}: {}",
            partition_path,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uuid.is_empty() {
        return Err(format!("blkid returned empty PARTUUID for {:?}", partition_path));
    }

    Ok(uuid)
}
