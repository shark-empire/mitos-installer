use crate::utils::run_chroot_command;
use log::info;
use std::path::Path;

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
    let sudoers_cmd =
        "sed -i 's/^# %wheel ALL=(ALL:ALL) ALL/%wheel ALL=(ALL:ALL) ALL/' /etc/sudoers";

    // We wrap sed in `sh -c` to ensure the shell handles the quotes properly inside the chroot
    run_chroot_command(target_mount, &format!("sh -c \"{}\"", sudoers_cmd), None)
        .map_err(|e| format!("Failed to configure sudoers: {}", e))?;

    Ok(())
}
