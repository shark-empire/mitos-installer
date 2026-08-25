use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

/// Configures the primary init system symlink for the kernel to execute
pub fn configure_init(target_mount: &Path, init_binary: &str) -> Result<(), String> {
    let sbin_dir = target_mount.join("sbin");
    let init_path = sbin_dir.join("init");

    // Ensure the /sbin directory exists in the target rootfs
    if !sbin_dir.exists() {
        fs::create_dir_all(&sbin_dir)
            .map_err(|e| format!("Failed to create /sbin directory: {}", e))?;
    }

    // If an init executable or symlink already exists, remove it to prevent conflicts
    if init_path.exists() || init_path.is_symlink() {
        fs::remove_file(&init_path)
            .map_err(|e| format!("Failed to remove existing /sbin/init: {}", e))?;
    }

    // Create a symlink from /sbin/init to the actual init binary (e.g., /usr/lib/systemd/systemd or /bin/busybox)
    // Note: The symlink target is absolute to the target OS root, not the live environment mount path.
    symlink(init_binary, &init_path)
        .map_err(|e| format!("Failed to symlink /sbin/init to {}: {}", init_binary, e))?;

    Ok(())
}
