use log::{error, info, warn};
use std::path::Path;
use std::process::Command;

/// Triggers emergency cleanup and rollback procedures after a critical installation failure
pub fn trigger_emergency_cleanup(target_mount: &Path, target_disk: Option<&Path>) {
    warn!("Critical failure detected. Initiating emergency rollback procedure...");

    // 1. Force unmount everything recursively under the target mount
    if target_mount.exists() {
        info!("Force unmounting target directories at {:?}", target_mount);

        // -R: recursive, -l: lazy (detaches immediately, cleans up when not busy)
        let umount_status = Command::new("umount")
            .args(["-R", "-l", target_mount.to_str().unwrap()])
            .status();

        match umount_status {
            Ok(status) if status.success() => info!("Target directories unmounted successfully."),
            Ok(status) => error!("umount returned non-zero status during cleanup: {}", status),
            Err(e) => error!("Failed to execute umount during cleanup: {}", e),
        }
    }

    // 2. Zap the partition table to prevent the system from trying to boot a broken OS
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

        // Force the kernel to recognize the wiped state
        let _ = Command::new("partprobe")
            .arg(disk.to_str().unwrap())
            .status();
    }

    warn!("Emergency rollback completed. System is safe to restart the installer.");
}
