// In profile.rs
pub enum InstallProfile {
    Minimal,
    Standard,
    Gaming,
    Creator,
}

pub fn deploy_profile(profile: InstallProfile, target_mount: &Path) -> Result<(), String> {
    // After rootfs is extracted, run the mitos package manager
    match profile {
        InstallProfile::Gaming => {
            // Chroot into the target and install gaming packages
            Command::new("chroot")
                .arg(target_mount)
                .args(["/usr/bin/mitos-pkg", "install", "steam", "lutris", "gamemode"])
                .status()?;
        },
        _ => {}
    }
    Ok(())
}
