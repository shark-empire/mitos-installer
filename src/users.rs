use std::fs;
use std::path::Path;
use std::process::Command;
use crate::utils::run_chroot_command;
use log::info;


pub fn configure_users(
    target_mount: &Path,
    username: &str,
    user_pass: &str,
    root_pass: &str,
) -> Result<(), String> {
    info!("Setting up root account...");
    // Format the credentials as "username:password" for chpasswd
    let root_credentials = format!("root:{}", root_pass);
    
    // We pass the credentials via stdin (the 3rd argument) to keep passwords out of ps/logs
    run_chroot_command(target_mount, "chpasswd", Some(&root_credentials))
        .map_err(|e| format!("Failed to set root password: {}", e))?;

    info!("Creating user '{}'...", username);
    // Create user: -m (create home dir), -s (default shell), -G wheel (add to admin group)
    let useradd_cmd = format!("useradd -m -s /bin/bash -G wheel {}", username);
    run_chroot_command(target_mount, &useradd_cmd, None)
        .map_err(|e| format!("Failed to create user '{}': {}", username, e))?;

    info!("Setting password for user '{}'...", username);
    let user_credentials = format!("{}:{}", username, user_pass);
    run_chroot_command(target_mount, "chpasswd", Some(&user_credentials))
        .map_err(|e| format!("Failed to set password for user '{}': {}", username, e))?;

    info!("Enabling sudo access for the 'wheel' group...");
    // Uncomment the wheel group in /etc/sudoers so the new user can use sudo
    let sudoers_cmd = "sed -i 's/^# %wheel ALL=(ALL:ALL) ALL/%wheel ALL=(ALL:ALL) ALL/' /etc/sudoers";
    
    // We wrap sed in `sh -c` to ensure the shell handles the quotes properly inside the chroot
    run_chroot_command(target_mount, &format!("sh -c \"{}\"", sudoers_cmd), None)
        .map_err(|e| format!("Failed to configure sudoers: {}", e))?;

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
