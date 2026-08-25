use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct KernelArtifacts {
    pub kernel_path: String, // e.g., "/boot/vmlinuz-mitos" or freestanding ELF
    pub initramfs_path: Option<String>, // e.g., "/boot/initramfs-mitos.img"
}

/// Copies kernel images to the target system's /boot directory
pub fn install_kernel_binaries(
    artifacts: &KernelArtifacts,
    target_dir: &Path,
) -> Result<(), String> {
    let boot_dir = target_dir.join("boot");

    fs::create_dir_all(&boot_dir)
        .map_err(|e| format!("Failed to create /boot in target: {}", e))?;

    // 1. Install Kernel Binary
    let src_kernel = Path::new(&artifacts.kernel_path);
    if !src_kernel.exists() {
        return Err(format!(
            "Kernel source file does not exist: {:?}",
            src_kernel
        ));
    }

    let kernel_name = src_kernel.file_name().ok_or("Invalid kernel filename")?;
    let dest_kernel = boot_dir.join(kernel_name);

    fs::copy(src_kernel, &dest_kernel)
        .map_err(|e| format!("Failed to copy kernel binary to {:?}: {}", dest_kernel, e))?;

    // 2. Optional Initramfs Deployment
    if let Some(ref initrd_path) = artifacts.initramfs_path {
        let src_initrd = Path::new(initrd_path);
        if src_initrd.exists() {
            let initrd_name = src_initrd.file_name().ok_or("Invalid initramfs filename")?;
            let dest_initrd = boot_dir.join(initrd_name);

            fs::copy(src_initrd, &dest_initrd)
                .map_err(|e| format!("Failed to copy initramfs to {:?}: {}", dest_initrd, e))?;
        }
    }

    Ok(())
}
