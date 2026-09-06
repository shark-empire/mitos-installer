use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_TARGET_MOUNT: &str = "/mnt/mitos";

/// Represents different types of mounts so we can unmount them in the exact reverse order.
#[derive(Debug, Clone)]
enum MountPoint {
    Root(PathBuf),
    Subvolume(PathBuf),
    Efi(PathBuf),
    Bind(PathBuf),
}

impl MountPoint {
    fn path(&self) -> &Path {
        match self {
            MountPoint::Root(p) => p,
            MountPoint::Subvolume(p) => p,
            MountPoint::Efi(p) => p,
            MountPoint::Bind(p) => p,
        }
    }
}

#[derive(Debug)]
pub struct MountGuard {
    pub target_dir: PathBuf,
    mounts: Vec<MountPoint>, // Tracks mounts in chronological order for safe teardown
}

impl MountGuard {
    pub fn new<P: AsRef<Path>>(target_dir: P) -> Self {
        Self {
            target_dir: target_dir.as_ref().to_path_buf(),
            mounts: Vec::new(),
        }
    }

    /// Mounts the root partition (or LUKS mapper). 
    /// Pass `is_btrfs = true` to apply modern Btrfs performance flags.
    pub fn mount_root(&mut self, root_part: &Path, is_btrfs: bool) -> Result<(), String> {
        fs::create_dir_all(&self.target_dir)
            .map_err(|e| format!("Failed to create directory {:?}: {}", self.target_dir, e))?;

        let mut cmd = Command::new("mount");
        
        // Apply modern Btrfs performance flags if applicable
        if is_btrfs {
            cmd.args(["-o", "subvol=@,compress=zstd:1,noatime,discard=async"]);
        }

        cmd.args([
            root_part.to_str().unwrap(),
            self.target_dir.to_str().unwrap(),
        ]);

        let status = cmd.status().map_err(|e| format!("Failed to execute mount command for root: {}", e))?;
        if !status.success() {
            return Err(format!("Failed to mount {:?} to {:?}", root_part, self.target_dir));
        }
        
        self.mounts.push(MountPoint::Root(self.target_dir.clone()));
        Ok(())
    }

    /// Mounts Btrfs subvolumes like @home, @var, etc.
    pub fn mount_btrfs_subvolume(&mut self, root_part: &Path, subvol_name: &str, target_subdir: &str) -> Result<(), String> {
        let target_path = self.target_dir.join(target_subdir);
        fs::create_dir_all(&target_path)
            .map_err(|e| format!("Failed to create subvolume dir {:?}: {}", target_path, e))?;

        let mount_opts = format!("subvol={},compress=zstd:1,noatime,discard=async", subvol_name);
        
        let status = Command::new("mount")
            .args(["-o", &mount_opts, root_part.to_str().unwrap(), target_path.to_str().unwrap()])
            .status()
            .map_err(|e| format!("Failed to mount subvolume {}: {}", subvol_name, e))?;

        if !status.success() {
            return Err(format!("Failed to mount subvolume {} to {:?}", subvol_name, target_path));
        }

        self.mounts.push(MountPoint::Subvolume(target_path));
        Ok(())
    }

    /// Mounts the EFI partition
    pub fn mount_efi(&mut self, efi_part: &Path) -> Result<(), String> {
        let efi_dir = self.target_dir.join("boot/efi");
        fs::create_dir_all(&efi_dir)
            .map_err(|e| format!("Failed to create EFI directory {:?}: {}", efi_dir, e))?;

        let status = Command::new("mount")
            .args([efi_part.to_str().unwrap(), efi_dir.to_str().unwrap()])
            .status()
            .map_err(|e| format!("Failed to execute mount command for EFI: {}", e))?;

        if !status.success() {
            return Err(format!("Failed to mount {:?} to {:?}", efi_part, efi_dir));
        }
        
        self.mounts.push(MountPoint::Efi(efi_dir));
        Ok(())
    }

    /// CRITICAL: Bind mounts pseudo-filesystems (/dev, /proc, /sys, /run).
    /// This is REQUIRED before running `chroot` commands (like systemd-machine-id setup, 
    /// bootloader installation, or generating the initramfs).
    pub fn mount_pseudo_filesystems(&mut self) -> Result<(), String> {
        let binds = [
            ("/dev", "dev"),
            ("/proc", "proc"),
            ("/sys", "sys"),
            ("/run", "run"),
        ];

        for (source, target_rel) in binds.iter() {
            let target_path = self.target_dir.join(target_rel);
            fs::create_dir_all(&target_path).unwrap_or_default();

            let status = Command::new("mount")
                .args(["--bind", source, target_path.to_str().unwrap()])
                .status()
                .map_err(|e| format!("Failed to bind mount {}: {}", source, e))?;

            if !status.success() {
                return Err(format!("Failed to bind mount {} to {:?}", source, target_path));
            }
            
            self.mounts.push(MountPoint::Bind(target_path));
        }

        // Specifically bind /dev/pts and /dev/shm as they are strictly needed by some chroot environments
        let dev_pts = self.target_dir.join("dev/pts");
        fs::create_dir_all(&dev_pts).unwrap_or_default();
        let _ = Command::new("mount").args(["--bind", "/dev/pts", dev_pts.to_str().unwrap()]).status();
        self.mounts.push(MountPoint::Bind(dev_pts));

        let dev_shm = self.target_dir.join("dev/shm");
        fs::create_dir_all(&dev_shm).unwrap_or_default();
        let _ = Command::new("mount").args(["--bind", "/dev/shm", dev_shm.to_str().unwrap()]).status();
        self.mounts.push(MountPoint::Bind(dev_shm));

        Ok(())
    }

    /// Safely unmounts everything in the exact reverse order they were mounted.
    /// This prevents the dreaded "target is busy" umount errors.
    pub fn unmount_all(&mut self) -> Result<(), String> {
        // Unmount in reverse order
        while let Some(mount_point) = self.mounts.pop() {
            let path = mount_point.path();
            
            // Use -R (recursive) to catch any lingering nested mounts
            let _ = Command::new("umount")
                .args(["-R", path.to_str().unwrap()])
                .status();
        }
        Ok(())
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        // Automatic cleanup on unexpected failure, panic, or exit
        let _ = self.unmount_all();
    }
}
