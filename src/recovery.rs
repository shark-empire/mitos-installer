//! Recovery mode: operates on an *existing* MITOS installation rather than creating a new
//! one. Covers the five actions from the installer spec: repair boot, repair packages,
//! reinstall system components, inspect logs, and restore configuration.

use crate::logging::TransactionLog;
use crate::mount::MountGuard;
use crate::{
    bootloader, config, encryption, filesystem, kernel, locale, network, platform, rootfs,
    security, utils,
};
use log::{error, info, warn};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const EFI_SYSTEM_PARTITION_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const BIOS_BOOT_PARTITION_GUID: &str = "21686148-6449-6e6f-744e-656564454649";

/// Conventional live-environment paths, matching what `installer.rs` uses for a fresh
/// install - recovery mode runs from the same live media, so the same rootfs/kernel
/// artifacts are expected to be available for "reinstall system components".
const LIVE_ROOTFS_ARCHIVE: &str = "/run/mitos-live/rootfs.tar";
const LIVE_KERNEL_IMAGE: &str = "/run/mitos-live/bzImage";

// ---------------------------------------------------------------------------------------
// Emergency rollback (used by a fresh install on failure, not by recovery mode itself)
// ---------------------------------------------------------------------------------------

/// Triggers emergency cleanup and rollback procedures after a critical installation
/// failure: deactivates any swap, force-unmounts the target, closes any open LUKS
/// mapping, then wipes the partition table so the disk is left in a clean, known state
/// for a retry rather than a half-partitioned unknown one.
pub fn trigger_emergency_cleanup(
    target_mount: &Path,
    target_disk: Option<&Path>,
    swap_partition: Option<&Path>,
    luks_mapper_name: Option<&str>,
) {
    warn!("Critical failure detected. Initiating emergency rollback procedure...");

    if let Some(swap) = swap_partition {
        info!("Deactivating swap on {:?} before rollback...", swap);
        filesystem::disable_swap(swap);
    }

    if target_mount.exists() {
        info!("Force unmounting target directories at {:?}", target_mount);
        let umount_status = Command::new("umount")
            .args(["-R", "-l", target_mount.to_str().unwrap()])
            .status();

        match umount_status {
            Ok(status) if status.success() => info!("Target directories unmounted successfully."),
            Ok(status) => error!("umount returned non-zero status during cleanup: {}", status),
            Err(e) => error!("Failed to execute umount during cleanup: {}", e),
        }
    }

    if let Some(mapper_name) = luks_mapper_name {
        info!("Closing LUKS mapping '{}' before rollback...", mapper_name);
        if let Err(e) = encryption::close_luks(mapper_name) {
            error!("Failed to close LUKS mapping '{}': {}", mapper_name, e);
        }
    }

    if let Some(disk) = target_disk {
        warn!(
            "Wiping partition table on {:?} to prevent corrupted boot state...",
            disk
        );

        let zap_status = Command::new("sgdisk")
            .args(["--zap-all", disk.to_str().unwrap()])
            .status();

        match zap_status {
            Ok(status) if status.success() => {
                info!("Successfully wiped partition table on {:?}", disk)
            }
            Ok(status) => error!(
                "sgdisk returned non-zero status during rollback: {}",
                status
            ),
            Err(e) => error!("Failed to execute sgdisk for rollback: {}", e),
        }

        let _ = Command::new("partprobe").arg(disk.to_str().unwrap()).status();
    }

    warn!("Emergency rollback completed. System is safe to restart the installer.");
}

// ---------------------------------------------------------------------------------------
// Discovering existing installations
// ---------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone, Default)]
struct LsblkNode {
    name: String,
    path: Option<PathBuf>,
    #[serde(default)]
    size: u64,
    #[serde(rename = "type", default)]
    dev_type: String,
    fstype: Option<String>,
    parttype: Option<String>,
    #[serde(default)]
    children: Vec<LsblkNode>,
}

#[derive(Debug, Deserialize)]
struct LsblkTree {
    blockdevices: Vec<LsblkNode>,
}

fn scan_block_devices() -> Result<Vec<LsblkNode>, String> {
    let output = Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,SIZE,TYPE,FSTYPE,PARTTYPE"])
        .output()
        .map_err(|e| format!("Failed to execute lsblk: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: LsblkTree =
        serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse lsblk JSON: {}", e))?;

    Ok(parsed.blockdevices)
}

/// A partition that looks like it might hold an existing MITOS (or MITOS-compatible)
/// installation, found by scanning every disk for ext4/btrfs/LUKS partitions.
#[derive(Debug, Clone)]
pub struct InstallationCandidate {
    pub disk_path: PathBuf,
    pub root_partition: PathBuf,
    pub esp_partition: Option<PathBuf>,
    pub bios_stage2_partition: Option<PathBuf>,
    pub fs_type_name: String,
    pub is_luks: bool,
    pub hostname: Option<String>,
    /// True if we were able to confirm `/etc/os-release` identifies this as MITOS
    /// (ID=mitos). Always false for LUKS candidates, since we can't peek inside without
    /// the passphrase - those are still listed, just with lower confidence.
    pub confirmed_mitos: bool,
}

impl InstallationCandidate {
    pub fn description(&self) -> String {
        let confidence = if self.confirmed_mitos {
            "confirmed MITOS"
        } else if self.is_luks {
            "encrypted, unverified"
        } else {
            "unverified"
        };
        let host = self.hostname.as_deref().unwrap_or("unknown hostname");
        format!(
            "{} ({}, {}, {})",
            self.root_partition.display(),
            self.fs_type_name,
            host,
            confidence
        )
    }
}

/// Scans every disk for partitions that could plausibly be an existing installation.
/// Read-only, non-destructive: candidates that aren't LUKS are briefly mounted read-only
/// to check for /etc/os-release and /etc/hostname, then immediately unmounted.
pub fn list_installation_candidates() -> Result<Vec<InstallationCandidate>, String> {
    let devices = scan_block_devices()?;
    let mut candidates = Vec::new();

    for disk in devices.iter().filter(|d| d.dev_type == "disk") {
        let Some(disk_path) = &disk.path else { continue };

        let esp_partition = disk
            .children
            .iter()
            .find(|c| matches_guid(c, EFI_SYSTEM_PARTITION_GUID) || c.fstype.as_deref() == Some("vfat"))
            .and_then(|c| c.path.clone());

        let bios_stage2_partition = disk
            .children
            .iter()
            .find(|c| matches_guid(c, BIOS_BOOT_PARTITION_GUID))
            .and_then(|c| c.path.clone());

        for part in &disk.children {
            let Some(part_path) = &part.path else { continue };
            let fstype = part.fstype.clone().unwrap_or_default();

            let is_luks = fstype == "crypto_LUKS";
            let is_plain_root_fs = fstype == "ext4" || fstype == "btrfs";
            if !is_luks && !is_plain_root_fs {
                continue;
            }

            let (hostname, confirmed_mitos) = if is_luks {
                (None, false)
            } else {
                probe_root_candidate(part_path)
            };

            candidates.push(InstallationCandidate {
                disk_path: disk_path.clone(),
                root_partition: part_path.clone(),
                esp_partition: esp_partition.clone(),
                bios_stage2_partition: bios_stage2_partition.clone(),
                fs_type_name: if is_luks {
                    "encrypted (crypto_LUKS)".to_string()
                } else {
                    fstype
                },
                is_luks,
                hostname,
                confirmed_mitos,
            });
        }
    }

    Ok(candidates)
}

fn matches_guid(node: &LsblkNode, guid: &str) -> bool {
    node.parttype
        .as_deref()
        .map(|p| p.eq_ignore_ascii_case(guid))
        .unwrap_or(false)
}

/// Briefly, read-only mounts `partition` to check for /etc/os-release (ID=mitos) and
/// /etc/hostname, then unmounts. Any failure here just means "unknown" - it never aborts
/// the overall scan.
fn probe_root_candidate(partition: &Path) -> (Option<String>, bool) {
    let probe_dir = PathBuf::from("/mnt/mitos-recovery-probe");
    if fs::create_dir_all(&probe_dir).is_err() {
        return (None, false);
    }

    let mount_status = Command::new("mount")
        .args(["-o", "ro", partition.to_str().unwrap_or(""), probe_dir.to_str().unwrap()])
        .status();

    let Ok(status) = mount_status else {
        return (None, false);
    };
    if !status.success() {
        return (None, false);
    }

    let hostname = fs::read_to_string(probe_dir.join("etc/hostname"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let confirmed_mitos = fs::read_to_string(probe_dir.join("etc/os-release"))
        .map(|contents| {
            contents
                .lines()
                .any(|line| line.trim().eq_ignore_ascii_case("id=mitos"))
        })
        .unwrap_or(false);

    let _ = Command::new("umount").arg(&probe_dir).status();

    (hostname, confirmed_mitos)
}

fn detect_fstype(partition: &Path) -> Result<String, String> {
    let output = Command::new("blkid")
        .args(["-p", "-s", "TYPE", "-o", "value", partition.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to execute blkid: {}", e))?;
    if !output.status.success() {
        return Err(format!("Could not determine filesystem type of {:?}", partition));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ---------------------------------------------------------------------------------------
// Opening a session on an existing installation
// ---------------------------------------------------------------------------------------

pub struct RecoverySession {
    pub disk_path: PathBuf,
    /// The raw root partition - if the install is encrypted, this is the LUKS container,
    /// *not* the opened mapper.
    pub raw_root_partition: PathBuf,
    pub esp_partition: Option<PathBuf>,
    pub bios_stage2_partition: Option<PathBuf>,
    pub is_luks: bool,
    pub mount_guard: MountGuard,
    pub hostname: Option<String>,
}

impl RecoverySession {
    pub fn target_mount(&self) -> &Path {
        &self.mount_guard.target_dir
    }
}

/// Opens (mounts) an installation candidate for repair. If it's LUKS-encrypted,
/// `luks_passphrase` must be `Some` - recovery.rs deliberately never prompts for input
/// itself, so the caller (the UI layer) is responsible for collecting it first.
pub fn open_session(
    candidate: &InstallationCandidate,
    luks_passphrase: Option<&str>,
) -> Result<RecoverySession, String> {
    let effective_root: PathBuf = if candidate.is_luks {
        let passphrase = luks_passphrase
            .ok_or("This installation is encrypted; a passphrase is required to open it.")?;
        encryption::open_luks(&candidate.root_partition, encryption::ROOT_MAPPER_NAME, passphrase)?;
        encryption::mapper_path(encryption::ROOT_MAPPER_NAME)
    } else {
        candidate.root_partition.clone()
    };

    let fstype = detect_fstype(&effective_root).unwrap_or_else(|_| "ext4".to_string());
    let is_btrfs = fstype == "btrfs";

    let mut guard = MountGuard::new("/mnt/mitos-recovery");
    if let Err(e) = guard.mount_root(&effective_root, is_btrfs) {
        if candidate.is_luks {
            let _ = encryption::close_luks(encryption::ROOT_MAPPER_NAME);
        }
        return Err(e);
    }

    if let Some(esp) = &candidate.esp_partition {
        guard.mount_efi(esp)?;
    }
    guard.mount_pseudo_filesystems()?;

    let hostname = fs::read_to_string(guard.target_dir.join("etc/hostname"))
        .ok()
        .map(|s| s.trim().to_string());

    Ok(RecoverySession {
        disk_path: candidate.disk_path.clone(),
        raw_root_partition: candidate.root_partition.clone(),
        esp_partition: candidate.esp_partition.clone(),
        bios_stage2_partition: candidate.bios_stage2_partition.clone(),
        is_luks: candidate.is_luks,
        mount_guard: guard,
        hostname,
    })
}

/// Explicitly unmounts and, if applicable, closes the LUKS mapping. `MountGuard` also
/// unmounts on drop, but doing it explicitly here lets us surface (log) any error instead
/// of silently discarding it, and lets us close the LUKS mapping only *after* the
/// filesystem is confirmed unmounted.
pub fn close_session(mut session: RecoverySession) {
    if let Err(e) = session.mount_guard.unmount_all() {
        warn!("Error while unmounting recovery session: {}", e);
    }
    if session.is_luks {
        if let Err(e) = encryption::close_luks(encryption::ROOT_MAPPER_NAME) {
            warn!("Error while closing LUKS mapping after recovery session: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Recovery action 1: repair boot
// ---------------------------------------------------------------------------------------

/// Regenerates the initramfs and reinstalls the bootloader, using whichever firmware mode
/// (UEFI/BIOS) this recovery session itself booted under.
pub fn repair_boot(session: &RecoverySession, txn_log: &TransactionLog) -> Result<(), String> {
    let target = session.target_mount().to_path_buf();
    let esp_partition = session
        .esp_partition
        .as_ref()
        .ok_or("No EFI System Partition was found on this disk; cannot repair the bootloader.")?;

    txn_log.record("recovery: repair_boot started");

    info!("Regenerating initramfs...");
    utils::run_chroot_command(&target, "dracut --force", None)?;

    let boot_mode = platform::detect_boot_mode();
    let esp_mount = target.join("boot/efi");

    let luks_boot_info = if session.is_luks {
        let uuid = encryption::luks_uuid(&session.raw_root_partition)?;
        Some(bootloader::LuksBootInfo {
            raw_partition_uuid: uuid,
            mapper_name: encryption::ROOT_MAPPER_NAME.to_string(),
        })
    } else {
        None
    };

    let req = bootloader::BootloaderRequest {
        disk_path: &session.disk_path,
        esp_mount: &esp_mount,
        esp_partition,
        bios_stage2_partition: session.bios_stage2_partition.as_deref(),
        root_partition: &session.raw_root_partition,
        kernel_name: kernel::DEFAULT_KERNEL_NAME,
        initramfs_name: kernel::DEFAULT_INITRAMFS_NAME,
        luks: luks_boot_info.as_ref(),
    };

    match boot_mode {
        platform::BootMode::Uefi => {
            info!("Reinstalling Limine for UEFI...");
            bootloader::install_limine_uefi(&req)?;
        }
        platform::BootMode::Bios => {
            info!("Reinstalling Limine for legacy BIOS...");
            bootloader::install_limine_bios(&req)?;
        }
    }

    bootloader::detect_and_add_windows(&esp_mount)?;

    txn_log.record("recovery: repair_boot completed");
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Recovery action 2: repair packages
// ---------------------------------------------------------------------------------------

/// Asks `mitos-pkg` to verify installed packages and reinstall anything broken. Exact
/// subcommands are a best effort (this repo doesn't ship `mitos-pkg`'s own docs) and
/// degrade gracefully: if it's missing entirely, this reports that clearly instead of
/// failing, since "reinstall system components" is the appropriate fallback in that case.
pub fn repair_packages(session: &RecoverySession, txn_log: &TransactionLog) -> Result<String, String> {
    let target = session.target_mount();
    txn_log.record("recovery: repair_packages started");

    if !target.join("usr/bin/mitos-pkg").exists() {
        let msg = "mitos-pkg is not present on this installation, so package integrity can't \
                    be checked automatically. Try 'Reinstall system components' instead."
            .to_string();
        txn_log.record("recovery: repair_packages skipped (mitos-pkg absent)");
        return Ok(msg);
    }

    info!("Running mitos-pkg verify...");
    let result = match utils::run_chroot_command(target, "mitos-pkg verify", None) {
        Ok(()) => "Package verification completed with no issues found.".to_string(),
        Err(verify_err) => {
            warn!(
                "mitos-pkg verify reported issues, attempting to reinstall affected packages: {}",
                verify_err
            );
            match utils::run_chroot_command(target, "mitos-pkg reinstall --broken", None) {
                Ok(()) => format!(
                    "Package verification found issues ({}). A reinstall of the affected \
                     packages was attempted.",
                    verify_err
                ),
                Err(reinstall_err) => {
                    return Err(format!(
                        "Package verification failed ({}), and the automatic reinstall attempt \
                         also failed ({}). Try 'Reinstall system components' instead.",
                        verify_err, reinstall_err
                    ))
                }
            }
        }
    };

    txn_log.record("recovery: repair_packages completed");
    Ok(result)
}

// ---------------------------------------------------------------------------------------
// Recovery action 3: reinstall system components
// ---------------------------------------------------------------------------------------

/// Re-extracts the base rootfs archive over the existing installation (repairing
/// corrupted or deleted system files without touching /home), then regenerates the
/// initramfs and re-applies security policies. Requires the live environment to have the
/// same rootfs archive available that a fresh install would use.
pub fn reinstall_system_components(
    session: &RecoverySession,
    txn_log: &TransactionLog,
) -> Result<(), String> {
    let target = session.target_mount().to_path_buf();
    txn_log.record("recovery: reinstall_system_components started");

    let archive = Path::new(LIVE_ROOTFS_ARCHIVE);
    if !archive.exists() {
        return Err(format!(
            "Rootfs archive not found at {:?}. Recovery mode expects to run from the same \
             live media used for installation.",
            archive
        ));
    }

    info!("Re-extracting base system files over the existing installation...");
    let source = rootfs::RootfsSource::Archive(archive.to_string_lossy().into_owned());
    rootfs::deploy_rootfs(&source, &target)?;

    let kernel_image = Path::new(LIVE_KERNEL_IMAGE);
    if kernel_image.exists() {
        info!("Reinstalling kernel...");
        let artifacts = kernel::KernelArtifacts {
            kernel_path: kernel_image.to_string_lossy().into_owned(),
            initramfs_path: None,
        };
        kernel::install_kernel_binaries(&artifacts, &target)?;
    } else {
        warn!(
            "Kernel image not found at {:?}; keeping the existing kernel in place.",
            kernel_image
        );
    }

    info!("Regenerating initramfs...");
    utils::run_chroot_command(&target, "dracut --force", None)?;

    info!("Re-applying security policies...");
    security::apply_security_policies(&target)?;

    txn_log.record("recovery: reinstall_system_components completed");
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Recovery action 4: inspect logs
// ---------------------------------------------------------------------------------------

/// Gathers the original installation's logs for display: the transaction log from *this*
/// recovery session, the transaction log copied onto the target during its original
/// install (if present), and the tail of the free-form install log likewise copied onto
/// the target. Returns formatted text rather than printing directly, so the UI layer
/// decides how to present it (paging, scrolling, etc).
pub fn inspect_logs(session: &RecoverySession, recovery_log: &TransactionLog) -> String {
    let mut report = String::new();

    report.push_str("=== This recovery session ===\n");
    match recovery_log.read_entries() {
        Ok(entries) if !entries.is_empty() => {
            for (ts, desc) in entries {
                report.push_str(&format!("  [{}] {}\n", ts, desc));
            }
        }
        Ok(_) => report.push_str("  (no entries yet)\n"),
        Err(e) => report.push_str(&format!("  (could not read recovery log: {})\n", e)),
    }

    let installed_transactions = session
        .target_mount()
        .join("var/log/mitos-install-transactions.log");
    report.push_str("\n=== Original installation transaction log ===\n");
    match fs::read_to_string(&installed_transactions) {
        Ok(contents) if !contents.trim().is_empty() => {
            let install_log = TransactionLog::new(&installed_transactions);
            match install_log.read_entries() {
                Ok(entries) => {
                    for (ts, desc) in entries {
                        report.push_str(&format!("  [{}] {}\n", ts, desc));
                    }
                }
                Err(_) => report.push_str(&contents),
            }
        }
        _ => report.push_str("  (not found on this installation)\n"),
    }

    let installed_log = session.target_mount().join("var/log/mitos-install.log");
    report.push_str("\n=== Original installation log (last 40 lines) ===\n");
    match fs::read_to_string(&installed_log) {
        Ok(contents) => {
            let lines: Vec<&str> = contents.lines().collect();
            let start = lines.len().saturating_sub(40);
            for line in &lines[start..] {
                report.push_str(line);
                report.push('\n');
            }
        }
        Err(_) => report.push_str("  (not found on this installation)\n"),
    }

    report
}

// ---------------------------------------------------------------------------------------
// Recovery action 5: restore configuration
// ---------------------------------------------------------------------------------------

/// Values to (re-)apply to an existing installation's core config files. Every field is
/// required (not `Option`) rather than "only change what's set", so the UI can pre-fill
/// prompts with the currently detected values: the person only needs to change what's
/// actually broken, and the write is a clean, idempotent overwrite either way.
pub struct RestoreConfigInput {
    pub hostname: String,
    pub timezone: String,
    pub locale: String,
    pub keyboard_layout: String,
    pub reset_network: bool,
}

pub fn restore_configuration(
    session: &RecoverySession,
    input: &RestoreConfigInput,
    txn_log: &TransactionLog,
) -> Result<(), String> {
    let target = session.target_mount();
    txn_log.record("recovery: restore_configuration started");

    info!("Restoring hostname...");
    config::update_hostname(target, &input.hostname)?;

    info!("Restoring locale and timezone...");
    locale::configure_locale(target, &input.locale, &input.timezone)?;

    info!("Restoring keyboard layout...");
    locale::configure_keyboard(target, &input.keyboard_layout)?;

    if input.reset_network {
        info!("Restoring default network configuration...");
        network::configure_network(target, None)?;
    }

    txn_log.record("recovery: restore_configuration completed");
    Ok(())
}
