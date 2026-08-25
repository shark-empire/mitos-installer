use crate::utils::run_chroot_command;
use std::path::Path;

/// Applies standard Linux security permissions to the target rootfs
pub fn apply_security_policies(target_mount: &Path) -> Result<(), String> {
    // Lock down the shadow file (contains password hashes)
    run_chroot_command(target_mount, "chmod 600 /etc/shadow", None)?;
    run_chroot_command(target_mount, "chown root:root /etc/shadow", None)?;

    // Lock down the sudoers file if it exists
    let sudoers_path = target_mount.join("etc/sudoers");
    if sudoers_path.exists() {
        run_chroot_command(target_mount, "chmod 440 /etc/sudoers", None)?;
    }

    // Lock down root directory
    run_chroot_command(target_mount, "chmod 700 /root", None)?;

    Ok(())
}
