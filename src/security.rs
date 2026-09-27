use crate::utils::run_chroot_command;
use log::info;
use std::path::Path;

/// Applies standard Linux security hardening to the target rootfs. Every step here only
/// acts on files that actually exist in the target, so this stays safe to run regardless
/// of exactly which optional components a given MITOS build ships.
pub fn apply_security_policies(target_mount: &Path) -> Result<(), String> {
    // Lock down the shadow/gshadow files (contain password hashes) and their backups,
    // which useradd/chpasswd/groupadd create automatically as /etc/shadow- etc.
    for (file, mode) in [
        ("etc/shadow", "600"),
        ("etc/shadow-", "600"),
        ("etc/gshadow", "600"),
        ("etc/gshadow-", "600"),
    ] {
        if target_mount.join(file).exists() {
            run_chroot_command(target_mount, &format!("chmod {} /{}", mode, file), None)?;
            run_chroot_command(target_mount, &format!("chown root:root /{}", file), None)?;
        }
    }

    // Lock down the sudoers file if it exists
    let sudoers_path = target_mount.join("etc/sudoers");
    if sudoers_path.exists() {
        run_chroot_command(target_mount, "chmod 440 /etc/sudoers", None)?;
    }

    // Lock down root's home directory
    run_chroot_command(target_mount, "chmod 700 /root", None)?;

    // If OpenSSH is present, disable direct root login over SSH - the admin account
    // created by users.rs has full sudo access, so root login isn't needed and is a
    // common brute-force target.
    let sshd_config = target_mount.join("etc/ssh/sshd_config");
    if sshd_config.exists() {
        info!("Disabling SSH root login in /etc/ssh/sshd_config...");
        let cmd = "sh -c \"grep -q '^PermitRootLogin' /etc/ssh/sshd_config && \
                   sed -i 's/^PermitRootLogin.*/PermitRootLogin no/' /etc/ssh/sshd_config || \
                   echo 'PermitRootLogin no' >> /etc/ssh/sshd_config\"";
        run_chroot_command(target_mount, cmd, None)?;
    }

    Ok(())
}
