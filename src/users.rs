use crate::utils::run_chroot_command;
use log::info;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Supplementary groups worth adding the new user to when the target rootfs actually
/// defines them, so the desktop session has working audio/video/input/etc access without
/// hardcoding assumptions about exactly which groups a given MITOS build ships.
const DESIRABLE_SUPPLEMENTARY_GROUPS: &[&str] = &[
    "wheel", "video", "audio", "input", "storage", "network", "disk", "plugdev", "lp",
    "scanner", "render",
];

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
    run_chroot_command(target_mount, "chpasswd -e", Some(&root_credentials))
        .map_err(|e| format!("Failed to set root password: {}", e))?;

    // `-f` makes this a no-op (exit 0) if wheel already exists, which it almost certainly
    // does given /etc/sudoers already references %wheel - this just guards against a base
    // rootfs variant that doesn't predefine it, so `useradd -G wheel` below can't fail.
    run_chroot_command(target_mount, "groupadd -f wheel", None)
        .map_err(|e| format!("Failed to ensure 'wheel' group exists: {}", e))?;

    let groups = supplementary_groups_present(target_mount);
    info!(
        "Creating user '{}' (groups: {})...",
        username,
        groups.join(",")
    );
    let useradd_cmd = if groups.is_empty() {
        format!("useradd -m -s /bin/bash {}", username)
    } else {
        format!("useradd -m -s /bin/bash -G {} {}", groups.join(","), username)
    };
    run_chroot_command(target_mount, &useradd_cmd, None)
        .map_err(|e| format!("Failed to create user '{}': {}", username, e))?;

    info!("Setting password for user '{}'...", username);
    let user_credentials = format!("{}:{}", username, user_pass);
    run_chroot_command(target_mount, "chpasswd -e", Some(&user_credentials))
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

/// Intersects `DESIRABLE_SUPPLEMENTARY_GROUPS` with what's actually defined in the
/// target's /etc/group, so we never pass `-G` a group name that doesn't exist (which
/// would make the whole `useradd` call fail). If /etc/group can't be read for some
/// reason, we fall back to just "wheel", since sudo access is essential and the group
/// existence was already separately guaranteed above.
fn supplementary_groups_present(target_mount: &Path) -> Vec<String> {
    let group_file = target_mount.join("etc/group");
    let Ok(contents) = fs::read_to_string(&group_file) else {
        return vec!["wheel".to_string()];
    };

    let existing: HashSet<&str> = contents
        .lines()
        .filter_map(|line| line.split(':').next())
        .collect();

    DESIRABLE_SUPPLEMENTARY_GROUPS
        .iter()
        .filter(|g| existing.contains(*g))
        .map(|g| g.to_string())
        .collect()
}
