use std::fs;
use std::path::Path;
use std::process::Command;
use crate::utils::run_chroot_command;


/// Configures default networking using systemd-networkd
pub fn configure_network(target_mount: &Path) -> Result<(), String> {
    let network_dir = target_mount.join("etc/systemd/network");
    
    // Ensure the systemd network config directory exists
    fs::create_dir_all(&network_dir)
        .map_err(|e| format!("Failed to create /etc/systemd/network: {}", e))?;

    // Create a basic wired DHCP fallback for en* and eth* interfaces
    let wired_network_content = "\
    [Match]\n\
    Name=en* eth*\n\
    \n\
    [Network]\n\
    DHCP=yes\n\
    ";

    fs::write(network_dir.join("20-wired.network"), wired_network_content)
        .map_err(|e| format!("Failed to write 20-wired.network config: {}", e))?;

    // Enable systemd-networkd and systemd-resolved via chroot
    run_chroot_command(
    target_mount, 
    "systemctl enable systemd-networkd systemd-resolved",
    None
)?;

    // Link /etc/resolv.conf to systemd-resolved's stub file
    let resolv_conf = target_mount.join("etc/resolv.conf");
    if resolv_conf.exists() || resolv_conf.is_symlink() {
        let _ = fs::remove_file(&resolv_conf);
    }
    
    std::os::unix::fs::symlink(
        "../run/systemd/resolve/stub-resolv.conf",
        &resolv_conf
    ).map_err(|e| format!("Failed to symlink systemd-resolved resolv.conf: {}", e))?;

    Ok(())
}


