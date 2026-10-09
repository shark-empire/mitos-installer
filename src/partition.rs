use log::info;
use nix::sys::statvfs::statvfs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A mebibyte, for the size math below.
const MIB: u64 = 1024 * 1024;
/// Below this, we refuse to partition: too small to hold a usable MITOS install once the
/// ESP (and, optionally, a BIOS boot stub and swap) are carved out.
const MIN_ROOT_MIB: u64 = 8 * 1024; // 8 GiB
const ESP_SIZE_MIB: u64 = 512;
const BIOS_BOOT_SIZE_MIB: u64 = 1;

/// What to carve out of the disk, decided by the user during setup (or the answer file).
#[derive(Debug, Clone, Copy, Default)]
pub struct PartitionOptions {
    /// Create a small unformatted GPT "BIOS boot" partition for Limine's legacy stage 2.
    /// Required when installing under legacy BIOS with a GPT disk; unused under UEFI.
    pub bios_boot: bool,
    /// Size, in MiB, of an optional dedicated swap partition. `None` means no swap.
    pub swap_mib: Option<u64>,
}

/// The concrete partitions created on the target disk. Any field the corresponding
/// `PartitionOptions` didn't ask for is `None`.
#[derive(Debug, Clone)]
pub struct PartitionLayout {
    pub bios_partition: Option<PathBuf>,
    pub efi_partition: PathBuf,
    pub swap_partition: Option<PathBuf>,
    pub root_partition: PathBuf,
}

/// Returns the total size, in bytes, of the whole disk at `disk_path` (not a partition).
pub fn disk_size_bytes(disk_path: &Path) -> Result<u64, String> {
    let output = Command::new("lsblk")
        .args([
            "-b",
            "-d",
            "-n",
            "-o",
            "SIZE",
            disk_path.to_str().unwrap_or(""),
        ])
        .output()
        .map_err(|e| format!("Failed to execute lsblk: {}", e))?;

    if !output.status.success() {
        return Err(format!("Failed to determine the size of {:?}", disk_path));
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("Could not parse size output for {:?}", disk_path))
}

/// Checks that `disk_size_bytes` is large enough for the requested layout, with a fixed
/// minimum root size. Intended to be called before the destructive confirmation prompt,
/// so the user finds out about an undersized disk before agreeing to wipe it.
pub fn validate_disk_capacity(disk_size_bytes: u64, opts: &PartitionOptions) -> Result<(), String> {
    let mut required_mib = ESP_SIZE_MIB + MIN_ROOT_MIB;
    if opts.bios_boot {
        required_mib += BIOS_BOOT_SIZE_MIB;
    }
    if let Some(swap_mib) = opts.swap_mib {
        required_mib += swap_mib;
    }

    let disk_size_mib = disk_size_bytes / MIB;
    if disk_size_mib < required_mib {
        return Err(format!(
            "The selected disk is too small for this configuration: {} MiB available, but \
             at least {} MiB is required (ESP {} MiB{}{}, root {} MiB minimum). Choose a \
             larger disk, disable swap, or reduce the swap size.",
            disk_size_mib,
            required_mib,
            ESP_SIZE_MIB,
            if opts.bios_boot {
                format!(", BIOS boot {} MiB", BIOS_BOOT_SIZE_MIB)
            } else {
                String::new()
            },
            opts.swap_mib
                .map(|s| format!(", swap {} MiB", s))
                .unwrap_or_default(),
            MIN_ROOT_MIB
        ));
    }

    Ok(())
}

/// Checks that the filesystem mounted at `mount_point` has at least `needed_bytes` of free
/// space, adding a small safety margin. Used before extracting the (potentially multi-GiB)
/// rootfs archive, so a marginal partition fails fast with a clear message instead of
/// midway through a long extraction with a confusing "No space left on device" error.
pub fn check_available_space(mount_point: &Path, needed_bytes: u64) -> Result<(), String> {
    let stats =
        statvfs(mount_point).map_err(|e| format!("Failed to statvfs {:?}: {}", mount_point, e))?;

    let available_bytes = stats.blocks_available() as u64 * stats.fragment_size() as u64;
    // 5% headroom for filesystem metadata/journal overhead beyond the raw payload size.
    let needed_with_margin = needed_bytes + (needed_bytes / 20);

    if available_bytes < needed_with_margin {
        return Err(format!(
            "Not enough free space on {:?}: {} MiB available, but the rootfs payload needs \
             approximately {} MiB.",
            mount_point,
            available_bytes / MIB,
            needed_with_margin / MIB
        ));
    }

    Ok(())
}

/// Wipes `disk_path` and lays out a fresh GPT partition table according to `opts`.
pub fn partition_target_disk(
    disk_path: &Path,
    opts: &PartitionOptions,
) -> Result<PartitionLayout, String> {
    wipe_partition_table(disk_path)?;
    let layout = create_gpt_layout(disk_path, opts)?;
    settle_udev();
    Ok(layout)
}

fn wipe_partition_table(disk_path: &Path) -> Result<(), String> {
    let disk_str = disk_path.to_str().ok_or("Invalid disk path")?;

    // 1. Wipe filesystem signatures to prevent ghost filesystems
    Command::new("wipefs")
        .args(["-a", disk_str])
        .output()
        .map_err(|e| format!("Failed to run wipefs: {}", e))?;

    // 2. Zap the GPT/MBR tables entirely
    let status = Command::new("sgdisk")
        .args(["--zap-all", disk_str])
        .status()
        .map_err(|e| format!("Failed to execute sgdisk zap: {}", e))?;

    if !status.success() {
        return Err(format!("Failed to wipe partition table on {:?}", disk_path));
    }

    Ok(())
}

fn create_gpt_layout(disk_path: &Path, opts: &PartitionOptions) -> Result<PartitionLayout, String> {
    let disk_str = disk_path.to_str().ok_or("Invalid disk path")?;

    let mut args: Vec<String> = vec!["--clear".to_string()];
    let mut part_num: u8 = 1;

    let mut bios_partition = None;
    if opts.bios_boot {
        args.push(format!("--new={}:0:+{}M", part_num, BIOS_BOOT_SIZE_MIB));
        args.push(format!("--typecode={}:ef02", part_num)); // BIOS boot partition GUID
        args.push(format!("--change-name={}:MITOS_BIOSBOOT", part_num));
        bios_partition = Some(get_partition_path(disk_path, part_num));
        part_num += 1;
    }

    let efi_num = part_num;
    args.push(format!("--new={}:0:+{}M", efi_num, ESP_SIZE_MIB));
    args.push(format!("--typecode={}:ef00", efi_num)); // EFI System Partition GUID
    args.push(format!("--change-name={}:MITOS_EFI", efi_num));
    part_num += 1;

    let mut swap_partition = None;
    if let Some(swap_mib) = opts.swap_mib {
        let swap_num = part_num;
        args.push(format!("--new={}:0:+{}M", swap_num, swap_mib));
        args.push(format!("--typecode={}:8200", swap_num)); // Linux swap GUID
        args.push(format!("--change-name={}:MITOS_SWAP", swap_num));
        swap_partition = Some(get_partition_path(disk_path, swap_num));
        part_num += 1;
    }

    let root_num = part_num;
    args.push(format!("--new={}:0:0", root_num)); // remaining space on the disk
    args.push(format!("--typecode={}:8300", root_num)); // Linux filesystem GUID
    args.push(format!("--change-name={}:MITOS_ROOT", root_num));

    info!("Writing GPT partition table to {:?}...", disk_path);
    let status = Command::new("sgdisk")
        .args(&args)
        .arg(disk_str)
        .status()
        .map_err(|e| format!("Failed to execute sgdisk partitioning: {}", e))?;

    if !status.success() {
        return Err(format!("Failed to create GPT layout on {:?}", disk_path));
    }

    let _ = Command::new("partprobe").arg(disk_str).status();

    Ok(PartitionLayout {
        bios_partition,
        efi_partition: get_partition_path(disk_path, efi_num),
        swap_partition,
        root_partition: get_partition_path(disk_path, root_num),
    })
}

/// Gives udev a moment to create the new /dev/<disk>N nodes after partprobe. Best-effort:
/// if udevadm isn't present (unlikely on a systemd-based live environment, but not
/// impossible), we just skip the wait rather than fail the install over it.
fn settle_udev() {
    let _ = Command::new("udevadm")
        .args(["settle", "--timeout=10"])
        .status();
}

/// Automatically handles standard block naming (sda -> sda1)
/// and NVMe/MMC block naming (nvme0n1 -> nvme0n1p1)
fn get_partition_path(disk: &Path, part_num: u8) -> PathBuf {
    let path_str = disk.to_string_lossy();
    let suffix =
        if path_str.contains("nvme") || path_str.contains("mmc") || path_str.contains("loop") {
            format!("p{}", part_num)
        } else {
            format!("{}", part_num)
        };

    PathBuf::from(format!("{}{}", path_str, suffix))
}
