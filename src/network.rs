use crate::utils::run_chroot_command;
use log::warn;
use std::fs;
use std::path::Path;

/// Configures default networking (wired + wireless) using systemd-networkd, and, if the
/// live-environment Wi-Fi setup captured credentials, carries them over via iwd so the
/// installed system can reconnect automatically on first boot.
pub fn configure_network(
    target_mount: &Path,
    wifi_credentials: Option<&(String, String)>,
) -> Result<(), String> {
    let network_dir = target_mount.join("etc/systemd/network");
    fs::create_dir_all(&network_dir)
        .map_err(|e| format!("Failed to create /etc/systemd/network: {}", e))?;

    // Wired DHCP fallback for en*/eth* interfaces
    let wired_network_content = "\
    [Match]\n\
    Name=en* eth*\n\
    \n\
    [Network]\n\
    DHCP=yes\n\
    ";
    fs::write(network_dir.join("20-wired.network"), wired_network_content)
        .map_err(|e| format!("Failed to write 20-wired.network config: {}", e))?;

    // Wireless DHCP for wl* interfaces. Association/authentication itself is handled by
    // iwd (enabled below), not by systemd-networkd.
    let wireless_network_content = "\
    [Match]\n\
    Name=wl*\n\
    \n\
    [Network]\n\
    DHCP=yes\n\
    ";
    fs::write(
        network_dir.join("25-wireless.network"),
        wireless_network_content,
    )
    .map_err(|e| format!("Failed to write 25-wireless.network config: {}", e))?;

    // Enable systemd-networkd and systemd-resolved via chroot
    run_chroot_command(
        target_mount,
        "systemctl enable systemd-networkd systemd-resolved",
        None,
    )?;

    // iwd is the natural Wi-Fi daemon to pair with systemd-networkd. This is best-effort:
    // only attempted if the rootfs actually ships the unit, and a failure to enable it is
    // logged rather than fatal, since wired-only machines don't need it at all.
    if target_mount
        .join("usr/lib/systemd/system/iwd.service")
        .exists()
    {
        if let Err(e) = run_chroot_command(target_mount, "systemctl enable iwd", None) {
            warn!("Could not enable iwd: {}", e);
        }
    }

    // Link /etc/resolv.conf to systemd-resolved's stub file
    let resolv_conf = target_mount.join("etc/resolv.conf");
    if resolv_conf.exists() || resolv_conf.is_symlink() {
        let _ = fs::remove_file(&resolv_conf);
    }
    std::os::unix::fs::symlink("../run/systemd/resolve/stub-resolv.conf", &resolv_conf)
        .map_err(|e| format!("Failed to symlink systemd-resolved resolv.conf: {}", e))?;

    if let Some((ssid, psk)) = wifi_credentials {
        if let Err(e) = carry_over_wifi_credentials(target_mount, ssid, psk) {
            warn!(
                "Could not carry over Wi-Fi credentials for '{}' to the installed system: {}. \
                 You'll need to reconnect manually after first boot.",
                ssid, e
            );
        }
    }

    Ok(())
}

/// Writes an iwd network profile so the installed system can automatically reconnect to
/// the Wi-Fi network the live environment was using during setup, without asking the
/// user to re-enter the password on first boot.
fn carry_over_wifi_credentials(target_mount: &Path, ssid: &str, psk: &str) -> Result<(), String> {
    let iwd_dir = target_mount.join("var/lib/iwd");
    fs::create_dir_all(&iwd_dir).map_err(|e| format!("Failed to create {:?}: {}", iwd_dir, e))?;

    let filename = iwd_profile_filename(ssid);
    let content = format!("[Security]\nPassphrase={}\n", psk);

    fs::write(iwd_dir.join(&filename), content)
        .map_err(|e| format!("Failed to write iwd profile {}: {}", filename, e))?;

    Ok(())
}

/// iwd names its profile files after the SSID: the plain SSID if it's made up only of
/// alphanumerics/'-'/'_', otherwise an '=' followed by the lowercase-hex-encoded SSID
/// bytes. See `iwd.network(5)`.
fn iwd_profile_filename(ssid: &str) -> String {
    let is_simple_name = !ssid.is_empty()
        && ssid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

    if is_simple_name {
        format!("{}.psk", ssid)
    } else {
        let hex: String = ssid.bytes().map(|b| format!("{:02x}", b)).collect();
        format!("={}.psk", hex)
    }
}
