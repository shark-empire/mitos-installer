use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_TARGET_MOUNT: &str = "/mnt/mitos";

#[derive(Debug)]
pub struct MountGuard {
    pub target_dir: PathBuf,
    pub efi_mounted: bool,
    pub root_mounted: bool,
}

impl MountGuard {
    pub fn new<P: AsRef<Path>>(target_dir: P) -> Self {
        Self {
            target_dir: target_dir.as_ref().to_path_buf(),
            efi_mounted: false,
            root_mounted: false,
        }
    }

    /// Mounts the root partition at target root and EFI partition at target /boot/efi
    pub fn mount_target(&mut self, root_part: &Path, efi_part: &Path) -> Result<(), String> {
        // 1. Create target base directory
        fs::create_dir_all(&self.target_dir)
            .map_err(|e| format!("Failed to create directory {:?}: {}", self.target_dir, e))?;

        // 2. Mount root filesystem
        let root_status = Command::new("mount")
            .args([
                root_part.to_str().unwrap(),
                self.target_dir.to_str().unwrap(),
            ])
            .status()
            .map_err(|e| format!("Failed to execute mount command for root: {}", e))?;

        if !root_status.success() {
            return Err(format!(
                "Failed to mount {:?} to {:?}",
                root_part, self.target_dir
            ));
        }
        self.root_mounted = true;

        // 3. Create EFI mount point inside target
        let efi_dir = self.target_dir.join("boot/efi");
        fs::create_dir_all(&efi_dir)
            .map_err(|e| format!("Failed to create EFI directory {:?}: {}", efi_dir, e))?;

        // 4. Mount EFI filesystem
        let efi_status = Command::new("mount")
            .args([efi_part.to_str().unwrap(), efi_dir.to_str().unwrap()])
            .status()
            .map_err(|e| format!("Failed to execute mount command for EFI: {}", e))?;

        if !efi_status.success() {
            return Err(format!("Failed to mount {:?} to {:?}", efi_part, efi_dir));
        }
        self.efi_mounted = true;

        Ok(())
    }

    /// Safely unmounts EFI and Root in reverse order
    pub fn unmount_all(&mut self) -> Result<(), String> {
        if self.efi_mounted {
            let efi_dir = self.target_dir.join("boot/efi");
            let _ = Command::new("umount")
                .args(["-R", efi_dir.to_str().unwrap()])
                .status();
            self.efi_mounted = false;
        }

        if self.root_mounted {
            let _ = Command::new("umount")
                .args(["-R", self.target_dir.to_str().unwrap()])
                .status();
            self.root_mounted = false;
        }

        Ok(())
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        // Automatic cleanup on unexpected failure or exit
        let _ = self.unmount_all();
    }
}
