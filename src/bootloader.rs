use log::{info, warn};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Kernel command-line parameters needed to unlock and mount an encrypted root at boot.
pub struct LuksBootInfo {
    /// UUID of the LUKS *container* itself (`cryptsetup luksUUID <raw partition>`), not
    /// the filesystem UUID inside it.
    pub raw_partition_uuid: String,
    /// The device-mapper name the container is opened as, e.g. "mitos-root".
    pub mapper_name: String,
}

/// Everything the bootloader installers need. Grouped into a struct because the two
/// install paths (UEFI, BIOS) share almost all of it, and there are enough `&Path`/`&str`
/// fields that plain positional parameters would be easy to mix up by accident.
pub struct BootloaderRequest<'a> {
    /// The whole target disk, e.g. /dev/sda or /dev/nvme0n1 (not a partition).
    pub disk_path: &'a Path,
    /// The ESP, already mounted at <target>/boot/efi.
    pub esp_mount: &'a Path,
    /// The raw ESP partition device, e.g. /dev/sda1. Used for the UEFI NVRAM entry.
    pub esp_partition: &'a Path,
    /// The small unformatted GPT partition for Limine's BIOS stage 2, if one was created.
    /// Only used by `install_limine_bios`.
    pub bios_stage2_partition: Option<&'a Path>,
    /// The raw root partition (never the LUKS mapper), used to compute PARTUUID for an
    /// unencrypted root and to sanity-check partition numbers.
    pub root_partition: &'a Path,
    pub kernel_name: &'a str,
    pub initramfs_name: &'a str,
    /// Present when the root filesystem lives inside a LUKS container.
    pub luks: Option<&'a LuksBootInfo>,
}

/// Directory (relative to the ESP root) where limine.conf, the kernel, and the initramfs
/// all live. This single location is readable by Limine under both UEFI and BIOS, per
/// upstream Limine's documented search paths (root, /limine, /boot, /boot/limine).
const LIMINE_BOOT_DIR: &str = "boot/limine";

pub fn limine_conf_path(esp_mount: &Path) -> PathBuf {
    esp_mount.join(LIMINE_BOOT_DIR).join("limine.conf")
}

/// Installs Limine for UEFI: copies the EFI application, creates a UEFI NVRAM boot entry,
/// and writes limine.conf plus the kernel/initramfs into the shared Limine boot directory.
pub fn install_limine_uefi(req: &BootloaderRequest) -> Result<(), String> {
    let efi_mitos_dir = req.esp_mount.join("EFI").join("mitos");
    let efi_fallback_dir = req.esp_mount.join("EFI").join("BOOT");
    fs::create_dir_all(&efi_mitos_dir)
        .map_err(|e| format!("Failed to create EFI directory {:?}: {}", efi_mitos_dir, e))?;
    fs::create_dir_all(&efi_fallback_dir).map_err(|e| {
        format!(
            "Failed to create fallback EFI directory {:?}: {}",
            efi_fallback_dir, e
        )
    })?;

    let limine_efi_paths = [
        "/usr/share/limine/BOOTX64.EFI",
        "/usr/lib/limine/BOOTX64.EFI",
        "/usr/lib/limine/bootx64.efi",
    ];
    let limine_src = find_first_existing(&limine_efi_paths).ok_or_else(|| {
        "Limine EFI binary not found in the live environment. Checked /usr/share/limine and \
         /usr/lib/limine."
            .to_string()
    })?;

    fs::copy(&limine_src, efi_mitos_dir.join("BOOTX64.EFI"))
        .map_err(|e| format!("Failed to copy Limine EFI binary: {}", e))?;
    fs::copy(&limine_src, efi_fallback_dir.join("BOOTX64.EFI"))
        .map_err(|e| format!("Failed to copy Limine fallback binary: {}", e))?;

    deploy_shared_boot_files(req)?;

    info!("Creating UEFI boot entry via efibootmgr...");
    let esp_part_num = partition_number(req.disk_path, req.esp_partition)?;
    create_uefi_boot_entry(req.disk_path, esp_part_num)?;

    Ok(())
}

/// Installs Limine for legacy BIOS: copies limine-bios.sys and runs `limine bios-install`,
/// plus writes limine.conf/kernel/initramfs into the same shared Limine boot directory
/// used by the UEFI path (so a disk could in principle boot either way).
pub fn install_limine_bios(req: &BootloaderRequest) -> Result<(), String> {
    deploy_shared_boot_files(req)?;

    let limine_bios_paths = [
        "/usr/share/limine/limine-bios.sys",
        "/usr/lib/limine/limine-bios.sys",
    ];
    let limine_bios_src = find_first_existing(&limine_bios_paths).ok_or_else(|| {
        "limine-bios.sys not found in the live environment. Checked /usr/share/limine and \
         /usr/lib/limine."
            .to_string()
    })?;

    let dest_dir = req.esp_mount.join(LIMINE_BOOT_DIR);
    fs::copy(&limine_bios_src, dest_dir.join("limine-bios.sys"))
        .map_err(|e| format!("Failed to copy limine-bios.sys: {}", e))?;

    let disk_str = req
        .disk_path
        .to_str()
        .ok_or("Invalid disk path")?
        .to_string();

    let mut args = vec!["bios-install".to_string(), disk_str];
    if let Some(stage2) = req.bios_stage2_partition {
        let stage2_num = partition_number(req.disk_path, stage2)?;
        args.push(stage2_num.to_string());
    }

    info!(
        "Installing Limine BIOS stage to {:?} (args: {:?})...",
        req.disk_path, args
    );
    let status = Command::new("limine")
        .args(&args)
        .status()
        .map_err(|e| format!("Failed to execute 'limine bios-install': {}", e))?;

    if !status.success() {
        return Err(format!(
            "'limine bios-install' failed for disk {:?}",
            req.disk_path
        ));
    }

    Ok(())
}

/// Copies the kernel/initramfs into the ESP's shared Limine boot directory and writes
/// limine.conf. Shared between the UEFI and BIOS install paths since both read from the
/// exact same files.
fn deploy_shared_boot_files(req: &BootloaderRequest) -> Result<(), String> {
    let boot_dir = req.esp_mount.join(LIMINE_BOOT_DIR);
    fs::create_dir_all(&boot_dir).map_err(|e| format!("Failed to create {:?}: {}", boot_dir, e))?;

    // kernel.rs installs the kernel/initramfs into the target rootfs's own /boot
    // directory first; from here (esp_mount = <target>/boot/efi) that's just "..".
    let root_boot_dir = req.esp_mount.parent().unwrap_or(req.esp_mount);

    let src_kernel = root_boot_dir.join(req.kernel_name);
    if src_kernel.exists() {
        fs::copy(&src_kernel, boot_dir.join(req.kernel_name))
            .map_err(|e| format!("Failed to copy kernel to ESP: {}", e))?;
    } else {
        warn!(
            "Kernel not found at {:?}. Limine will fail to boot until this is fixed.",
            src_kernel
        );
    }

    let src_initrd = root_boot_dir.join(req.initramfs_name);
    if src_initrd.exists() {
        fs::copy(&src_initrd, boot_dir.join(req.initramfs_name))
            .map_err(|e| format!("Failed to copy initrd to ESP: {}", e))?;
    } else {
        warn!(
            "Initramfs not found at {:?}. Limine will fail to boot until this is fixed.",
            src_initrd
        );
    }

    let cmdline = build_kernel_cmdline(req.root_partition, req.luks)?;
    let limine_conf_content = format!(
        "timeout: 3\n\
         default_entry: 1\n\
         \n\
         /MITOS Linux\n\
             protocol: linux\n\
             kernel_path: boot():/{dir}/{kernel}\n\
             module_path: boot():/{dir}/{initrd}\n\
             cmdline: {cmdline}\n",
        dir = LIMINE_BOOT_DIR,
        kernel = req.kernel_name,
        initrd = req.initramfs_name,
        cmdline = cmdline
    );

    fs::write(boot_dir.join("limine.conf"), limine_conf_content)
        .map_err(|e| format!("Failed to write limine.conf: {}", e))?;

    Ok(())
}

/// Builds the `root=`/`rd.luks.*` portion of the kernel command line, branching on
/// whether the root filesystem is encrypted.
fn build_kernel_cmdline(
    raw_root_partition: &Path,
    luks: Option<&LuksBootInfo>,
) -> Result<String, String> {
    match luks {
        Some(info) => Ok(format!(
            "root=/dev/mapper/{mapper} rw quiet splash rd.luks.uuid={uuid} \
             rd.luks.name={uuid}={mapper} rd.luks.allow-discards",
            mapper = info.mapper_name,
            uuid = info.raw_partition_uuid
        )),
        None => {
            let partuuid = get_partuuid(raw_root_partition)?;
            Ok(format!("root=PARTUUID={} rw quiet splash", partuuid))
        }
    }
}

/// Helper to create a native UEFI boot entry in NVRAM
fn create_uefi_boot_entry(disk_path: &Path, esp_part_num: u8) -> Result<(), String> {
    let disk_str = disk_path.to_str().ok_or("Invalid disk path")?;

    let status = Command::new("efibootmgr")
        .args([
            "--create",
            "--disk",
            disk_str,
            "--part",
            &esp_part_num.to_string(),
            "--label",
            "MITOS",
            "--loader",
            r"\EFI\mitos\BOOTX64.EFI",
        ])
        .status()
        .map_err(|e| format!("Failed to execute efibootmgr: {}", e))?;

    if !status.success() {
        warn!(
            "efibootmgr failed. The system will rely on the fallback /EFI/BOOT/BOOTX64.EFI path."
        );
    }

    Ok(())
}

/// Given a partition device path (e.g. /dev/nvme0n1p2) and the whole disk it belongs to
/// (e.g. /dev/nvme0n1), returns the 1-based partition number (2). Handles both plain
/// (sdaN) and 'p'-separated (nvme0n1pN / mmcblk0pN) naming schemes - the exact inverse of
/// how `partition::get_partition_path` builds these paths in the first place.
fn partition_number(disk_path: &Path, partition_path: &Path) -> Result<u8, String> {
    let disk_str = disk_path.to_str().ok_or("Invalid disk path")?;
    let part_str = partition_path.to_str().ok_or("Invalid partition path")?;

    let suffix = part_str.strip_prefix(disk_str).ok_or_else(|| {
        format!(
            "{:?} does not look like a partition of {:?}",
            partition_path, disk_path
        )
    })?;
    let digits = suffix.trim_start_matches('p');

    digits.parse::<u8>().map_err(|_| {
        format!(
            "Could not parse a partition number from {:?} relative to disk {:?}",
            partition_path, disk_path
        )
    })
}

fn find_first_existing(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(Path::new)
        .find(|p| p.exists())
        .map(|p| p.to_path_buf())
}

/// Helper function to retrieve the PARTUUID of a given partition using `blkid`
fn get_partuuid(partition_path: &Path) -> Result<String, String> {
    let output = Command::new("blkid")
        .args([
            "-p",
            "-s",
            "PARTUUID",
            "-o",
            "value",
            partition_path.to_str().unwrap(),
        ])
        .output()
        .map_err(|e| format!("Failed to execute blkid: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to retrieve PARTUUID for {:?}: {}",
            partition_path,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uuid.is_empty() {
        return Err(format!(
            "blkid returned empty PARTUUID for {:?}",
            partition_path
        ));
    }

    Ok(uuid)
}

/// Detects Windows on the ESP and adds a chainload entry to limine.conf. This only
/// applies to UEFI Windows installs (identified by the presence of the standard
/// bootmgfw.efi path); legacy-BIOS Windows installs use a different, NTFS-boot-sector
/// based mechanism that this installer does not attempt to detect or chainload.
pub fn detect_and_add_windows(esp_mount: &Path) -> Result<(), String> {
    let windows_boot = esp_mount.join("EFI/Microsoft/Boot/bootmgfw.efi");

    if windows_boot.exists() {
        info!("Windows detected! Adding to Limine...");
        let windows_entry = "\n/Windows\n    protocol: efi_chainload\n    image_path: boot():/EFI/Microsoft/Boot/bootmgfw.efi\n";

        fs::OpenOptions::new()
            .append(true)
            .open(limine_conf_path(esp_mount))
            .and_then(|mut f| f.write_all(windows_entry.as_bytes()))
            .map_err(|e| format!("Failed to append Windows entry to limine.conf: {}", e))?;
    } else {
        info!("No Windows installation detected on this EFI partition.");
    }
    Ok(())
}
