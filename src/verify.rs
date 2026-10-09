use crate::utils::command_exists;
use nix::unistd::Uid;

/// Tools required for every installation, regardless of the chosen options.
const CORE_TOOLS: &[&str] = &[
    "sgdisk", "wipefs", "partprobe", "mkfs.vfat", "mkfs.ext4", "mount", "umount", "chroot",
    "blkid", "useradd", "chpasswd", "openssl", "tar", "lsblk", "sed",
];

/// Options that affect which tools are actually needed, so we don't fail an installation
/// over a missing tool (e.g. `cryptsetup`) that the user never asked to use.
#[derive(Debug, Clone, Copy)]
pub struct PrerequisiteOptions {
    pub uses_btrfs: bool,
    pub is_uefi: bool,
    pub wants_encryption: bool,
    pub wants_swap: bool,
}

/// Validates that the installation environment meets all prerequisites: running as root,
/// and every external tool the selected options will need is present on $PATH. Checking
/// this up front - before any disk is touched - means a missing tool produces a clean
/// error instead of a partially-partitioned disk.
pub fn check_prerequisites(opts: &PrerequisiteOptions) -> Result<(), String> {
    check_root_privileges()?;
    check_required_tools(opts)?;
    Ok(())
}

fn check_root_privileges() -> Result<(), String> {
    if !Uid::effective().is_root() {
        return Err("The MITOS installer must be run as root (UID 0).".to_string());
    }
    Ok(())
}

fn check_required_tools(opts: &PrerequisiteOptions) -> Result<(), String> {
    let mut missing: Vec<&str> = CORE_TOOLS
        .iter()
        .copied()
        .filter(|tool| !command_exists(tool))
        .collect();

    if opts.uses_btrfs && !command_exists("mkfs.btrfs") {
        missing.push("mkfs.btrfs");
    }
    if opts.is_uefi && !command_exists("efibootmgr") {
        missing.push("efibootmgr");
    }
    if !command_exists("limine") {
        missing.push("limine");
    }
    if opts.wants_encryption && !command_exists("cryptsetup") {
        missing.push("cryptsetup");
    }
    if opts.wants_swap && !command_exists("mkswap") {
        missing.push("mkswap");
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "The live environment is missing required tool(s): {}. The MITOS live image \
             should ship these; please report this as a bug.",
            missing.join(", ")
        ))
    }
}
