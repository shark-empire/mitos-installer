use crate::utils::run_chroot_command;
use log::{info, warn};
use std::path::Path;

/// Optional package bundles installed on top of the base MITOS rootfs via `mitos-pkg`.
/// The base rootfs (extracted in `rootfs.rs`) already contains a complete, working
/// system; everything here is additive and, aside from `Minimal`, network-dependent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstallProfile {
    /// No extra packages. Fastest, works fully offline.
    Minimal,
    /// A sensible set of everyday desktop applications.
    #[default]
    Standard,
    /// Standard, plus gaming-oriented packages.
    Gaming,
    /// Standard, plus creative/content-production packages.
    Creator,
}

impl InstallProfile {
    /// Package list passed to `mitos-pkg install`. Empty means "nothing to do".
    fn packages(self) -> &'static [&'static str] {
        match self {
            InstallProfile::Minimal => &[],
            InstallProfile::Standard => &["firefox", "file-roller", "gnome-text-editor"],
            InstallProfile::Gaming => &[
                "firefox",
                "file-roller",
                "gnome-text-editor",
                "steam",
                "lutris",
                "gamemode",
                "mangohud",
            ],
            InstallProfile::Creator => &[
                "firefox",
                "file-roller",
                "gnome-text-editor",
                "gimp",
                "inkscape",
                "blender",
                "obs-studio",
            ],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            InstallProfile::Minimal => "Minimal (base system only)",
            InstallProfile::Standard => "Standard (recommended desktop apps)",
            InstallProfile::Gaming => "Gaming (Standard + Steam, Lutris, GameMode)",
            InstallProfile::Creator => "Creator (Standard + GIMP, Inkscape, Blender, OBS)",
        }
    }

    pub const ALL: [InstallProfile; 4] = [
        InstallProfile::Minimal,
        InstallProfile::Standard,
        InstallProfile::Gaming,
        InstallProfile::Creator,
    ];
}

/// Which target systemd should boot into, and whether the graphical session is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesktopChoice {
    /// Boot straight to the MITOS graphical session (`mitos-session` / `mitos-gui`).
    #[default]
    Graphical,
    /// Headless/server install: boot to a text console, no graphical session enabled.
    HeadlessServer,
}

impl DesktopChoice {
    pub fn label(self) -> &'static str {
        match self {
            DesktopChoice::Graphical => "Desktop (graphical login, mitos-gui)",
            DesktopChoice::HeadlessServer => "Server (text console only, no GUI)",
        }
    }

    pub const ALL: [DesktopChoice; 2] = [DesktopChoice::Graphical, DesktopChoice::HeadlessServer];
}

/// Installs the packages for `profile` into the target rootfs using `mitos-pkg`.
///
/// This is best-effort: package installation needs working network access from the live
/// environment, which may not be available or may be flaky. A failure here is logged as
/// a warning and does not abort the overall installation, since the base system deployed
/// by `rootfs::deploy_rootfs` is already complete and bootable on its own.
pub fn deploy_profile(profile: InstallProfile, target_mount: &Path) -> Result<(), String> {
    let packages = profile.packages();
    if packages.is_empty() {
        info!(
            "Install profile '{}' adds no extra packages.",
            profile.label()
        );
        return Ok(());
    }

    if !target_mount.join("usr/bin/mitos-pkg").exists() {
        warn!("mitos-pkg not found in target rootfs; skipping package profile installation.");
        return Ok(());
    }

    info!(
        "Installing {} package(s) for profile '{}': {}",
        packages.len(),
        profile.label(),
        packages.join(", ")
    );

    let command = format!("mitos-pkg install -y {}", packages.join(" "));
    if let Err(e) = run_chroot_command(target_mount, &command, None) {
        warn!(
            "Package profile installation reported an error (continuing anyway, the base \
             system is still complete): {}",
            e
        );
    }

    Ok(())
}

/// Configures which systemd target the system boots to, and enables/disables the
/// graphical session accordingly. Uses `systemctl set-default`, which is a standard
/// mechanism and doesn't require knowing precise MITOS-specific unit names.
pub fn apply_desktop_choice(choice: DesktopChoice, target_mount: &Path) -> Result<(), String> {
    let target_name = match choice {
        DesktopChoice::Graphical => "graphical.target",
        DesktopChoice::HeadlessServer => "multi-user.target",
    };

    info!("Setting default systemd target to '{}'...", target_name);
    run_chroot_command(
        target_mount,
        &format!("systemctl set-default {}", target_name),
        None,
    )?;

    // Best-effort: if the rootfs ships a dedicated MITOS session/display-manager unit,
    // enable or disable it to match the choice. Missing units are not an error - the
    // rootfs may handle this purely through the default target already.
    for unit in ["mitos-session.service", "mitos-gui.service"] {
        let unit_path = target_mount.join("usr/lib/systemd/system").join(unit);
        if !unit_path.exists() {
            continue;
        }
        let action = match choice {
            DesktopChoice::Graphical => "enable",
            DesktopChoice::HeadlessServer => "disable",
        };
        if let Err(e) = run_chroot_command(
            target_mount,
            &format!("systemctl {} {}", action, unit),
            None,
        ) {
            warn!("Could not {} {}: {}", action, unit, e);
        }
    }

    Ok(())
}
