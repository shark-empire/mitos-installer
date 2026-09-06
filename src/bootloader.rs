use log::{info, warn};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

/// Installs Limine EFI binaries, creates UEFI boot entries, and writes the configuration file
pub fn install_limine(
    efi_mount: &Path,
    root_partition: &Path,
    kernel_name: &str, 
    initramfs_name: &str, 
) -> Result<(), String> {
    
    // 1. Create the MITOS EFI directory (Standard practice is /EFI/<distro>/)
    let efi_mitos_dir = efi_mount.join("EFI").join("mitos");
    let efi_fallback_dir = efi_mount.join("EFI").join("BOOT");
    
    fs::create_dir_all(&efi_mitos_dir)
        .map_err(|e| format!("Failed to create EFI directory {:?}: {}", efi_mitos_dir, e))?;
    fs::create_dir_all(&efi_fallback_dir)
        .map_err(|e| format!("Failed to create fallback EFI directory {:?}: {}", efi_fallback_dir, e))?;

    // 2. Locate and copy the Limine EFI binary
    let limine_paths = [
        "/usr/share/limine/BOOTX64.EFI",
        "/usr/lib/limine/BOOTX64.EFI",
        "/usr/lib/limine/bootx64.efi",
    ];
    
    let mut limine_src = None;
    for p in limine_paths.iter() {
        let path = Path::new(p);
        if path.exists() {
            limine_src = Some(path);
            break;
        }
    }
    
    let limine_src = limine_src.ok_or_else(|| 
        "Limine EFI binary not found in the live environment. Checked /usr/share/limine and /usr/lib/limine.".to_string()
    )?;

    let limine_dest = efi_mitos_dir.join("BOOTX64.EFI");
    let limine_fallback_dest = efi_fallback_dir.join("BOOTX64.EFI");

    fs::copy(limine_src, &limine_dest)
        .map_err(|e| format!("Failed to copy Limine EFI binary: {}", e))?;
    
    fs::copy(limine_src, &limine_fallback_dest)
        .map_err(|e| format!("Failed to copy Limine fallback binary: {}", e))?;

    // 3. CRITICAL FIX: Move Kernel and Initrd to the ESP
    // Limine's `boot():` protocol looks on the EFI partition. 
    // Since kernel.rs put them in the root /boot, we must copy them to the ESP.
    let root_boot_dir = efi_mount.parent().unwrap_or(efi_mount); // Gets <target>/boot
    
    let src_kernel = root_boot_dir.join(kernel_name);
    let dest_kernel = efi_mitos_dir.join(kernel_name);
    if src_kernel.exists() {
        fs::copy(&src_kernel, &dest_kernel)
            .map_err(|e| format!("Failed to copy kernel to ESP: {}", e))?;
    } else {
        warn!("Kernel not found at {:?}. Limine might fail to boot.", src_kernel);
    }

    let src_initrd = root_boot_dir.join(initramfs_name);
    let dest_initrd = efi_mitos_dir.join(initramfs_name);
    if src_initrd.exists() {
        fs::copy(&src_initrd, &dest_initrd)
            .map_err(|e| format!("Failed to copy initrd to ESP: {}", e))?;
    }

    // 4. Create UEFI NVRAM Boot Entry using efibootmgr
    info!("Creating UEFI boot entry via efibootmgr...");
    create_uefi_boot_entry(root_partition)?;

    // 5. Extract the PARTUUID of the target root partition
    let partuuid = get_partuuid(root_partition)?;

    // 6. Generate limine.conf
    let limine_conf_content = format!(
        "timeout: 3\n\
         default_entry: 1\n\
         \n\
         /MITOS Linux\n\
             protocol: linux\n\
             kernel_path: boot():/mitos/{kernel}\n\
             module_path: boot():/mitos/{initrd}\n\
             cmdline: root=PARTUUID={partuuid} rw quiet splash\n",
        kernel = kernel_name,
        initrd = initramfs_name,
        partuuid = partuuid
    );

    let conf_dest = efi_mitos_dir.join("limine.conf");
    fs::write(&conf_dest, limine_conf_content)
        .map_err(|e| format!("Failed to write limine.conf: {}", e))?;

    Ok(())
}

/// Helper to create a native UEFI boot entry in NVRAM
fn create_uefi_boot_entry(root_partition: &Path) -> Result<(), String> {
    let part_str = root_partition.to_str().unwrap();
    
    let disk_name = String::from_utf8_lossy(
        &Command::new("lsblk")
            .args(["-ndo", "PKNAME", part_str])
            .output()
            .map_err(|e| format!("Failed to run lsblk: {}", e))?
            .stdout
    ).trim().to_string();

    if disk_name.is_empty() {
        warn!("Could not determine parent disk for {}. Skipping UEFI NVRAM entry.", part_str);
        return Ok(());
    }

    let disk_path = format!("/dev/{}", disk_name);
    let part_num = part_str
        .strip_prefix(&disk_path)
        .unwrap_or("")
        .trim_start_matches('p')
        .trim_start_matches('/');

    // FIX: Use .as_str() to ensure all array elements are exactly `&str`
    let disk_path_str = disk_path.as_str();
    
    let status = Command::new("efibootmgr")
        .args([
            "--create",
            "--disk", disk_path_str,
            "--part", part_num,
            "--label", "MITOS",
            "--loader", r"\EFI\mitos\BOOTX64.EFI"
        ])
        .status()
        .map_err(|e| format!("Failed to execute efibootmgr: {}", e))?;

    if !status.success() {
        warn!("efibootmgr failed. The system will rely on the fallback /EFI/BOOT/BOOTX64.EFI path.");
    }

    Ok(())
}

/// Helper function to retrieve the PARTUUID of a given partition using `blkid`
fn get_partuuid(partition_path: &Path) -> Result<String, String> {
    let output = Command::new("blkid")
        .args([
            "-p",
            "-s", "PARTUUID",
            "-o", "value",
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

/// Detects Windows on the same EFI partition and adds it to Limine
pub fn detect_and_add_windows(efi_mount: &Path, limine_conf_path: &Path) -> Result<(), String> {
    let windows_boot = efi_mount.join("EFI/Microsoft/Boot/bootmgfw.efi");
    
    if windows_boot.exists() {
        info!("Windows detected! Adding to Limine...");
        let windows_entry = "\n/Windows\n    protocol: efi_chainload\n    image_path: boot():/EFI/Microsoft/Boot/bootmgfw.efi\n";
        
        fs::OpenOptions::new()
            .append(true)
            .open(limine_conf_path)
            .and_then(|mut f| f.write_all(windows_entry.as_bytes()))
            .map_err(|e| format!("Failed to append Windows entry to limine.conf: {}", e))?;
    } else {
        info!("No Windows installation detected on this EFI partition.");
    }
    Ok(())
}
