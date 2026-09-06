use log::info;
use std::path::{Path, PathBuf};

// Ensure 'utils' is imported for chroot commands
use crate::{
    bootloader, config, filesystem, hardware, init, kernel, locale, mount::MountGuard, network,
    partition, platform, rootfs, security, users, utils, verify,
};

#[derive(Debug, Clone, Default)]
pub struct SystemConfig {
    pub hostname: String,
    pub username: String,
    pub password_hash: String,
    pub timezone: String,
    pub locale: String,
}

#[derive(Debug, Clone)]
pub struct TargetDisk {
    pub device_path: PathBuf, // e.g., /dev/nvme0n1
    pub efi_partition: PathBuf,
    pub root_partition: PathBuf,
    pub mount_point: PathBuf, // e.g., /mnt/mitos
}

#[derive(Debug)]
pub struct InstallationContext {
    pub target: Option<TargetDisk>,
    pub sys_config: SystemConfig,
    pub is_uefi: bool,
    pub fs_type: config::FilesystemType, // NEW: Tracks the chosen filesystem
}

pub struct InstallerPipeline {
    pub ctx: InstallationContext, // Made public so the UI can modify it before execution
}

impl InstallerPipeline {
    pub fn new() -> Self {
        Self {
            ctx: InstallationContext {
                target: None,
                sys_config: SystemConfig::default(),
                is_uefi: false,
                fs_type: config::FilesystemType::Ext4, // Default to Ext4; UI can change this
            },
        }
    }

    pub fn execute(&mut self) -> Result<(), String> {
        info!("Starting MITOS Installation Pipeline...");

        // 1. Validate Context
        let target = self
            .ctx
            .target
            .as_mut()
            .ok_or("Target disk not configured in context.")?;

        // 2. Verification & Platform Checks
        info!("Step 1: Verifying system prerequisites...");
        platform::detect_platform(); 
        hardware::check_minimum_requirements()?; 
        verify::check_prerequisites()?;
        self.ctx.is_uefi = true;

        // 3. Partitioning
        info!("Step 2: Partitioning disk {:?}...", target.device_path);
        let layout = partition::partition_target_disk(&target.device_path)?;

        target.efi_partition = layout.efi_partition.clone();
        target.root_partition = layout.root_partition.clone();

        // 4. Formatting (WIRED: Btrfs Layout Creation)
        info!("Step 3: Formatting partitions...");
        filesystem::format_efi_partition(&target.efi_partition)?;
        
        let is_btrfs = self.ctx.fs_type == config::FilesystemType::Btrfs;
        
        if is_btrfs {
            info!("Creating Btrfs subvolume layout...");
            filesystem::create_btrfs_layout(&target.root_partition)?;
        } else {
            filesystem::format_root_partition(&target.root_partition, self.ctx.fs_type)?;
        }

        // 5. Mounting (WIRED: RAII Subvolumes & Pseudo-filesystems)
        info!("Step 4: Mounting filesystems to {:?}...", target.mount_point);
        let mut mount_guard = MountGuard::new(&target.mount_point);
        
        mount_guard.mount_root(&target.root_partition, is_btrfs)?;
        
        if is_btrfs {
            info!("Mounting Btrfs subvolumes...");
            mount_guard.mount_btrfs_subvolume(&target.root_partition, "@home", "home")?;
            mount_guard.mount_btrfs_subvolume(&target.root_partition, "@var", "var")?;
            mount_guard.mount_btrfs_subvolume(&target.root_partition, "@log", "var/log")?;
        }
        
        mount_guard.mount_efi(&target.efi_partition)?;

        // 6. Payload Deployment
        let rootfs_archive = Path::new("/run/mitos-live/rootfs.tar");
        let kernel_image = Path::new("/run/mitos-live/bzImage");

        info!("Step 5: Unpacking root filesystem...");
        let rootfs_source =
            rootfs::RootfsSource::Archive(rootfs_archive.to_string_lossy().into_owned());
        rootfs::deploy_rootfs(&rootfs_source, &target.mount_point)?;

        // CRITICAL: Bind pseudo-filesystems BEFORE ANY chroot commands
        info!("Step 5.5: Binding pseudo-filesystems for chroot environment...");
        mount_guard.mount_pseudo_filesystems()?;

        info!("Step 6: Deploying MITOS kernel...");
        let efi_mount = target.mount_point.join("boot/efi");
        let kernel_artifacts = kernel::KernelArtifacts {
            kernel_path: kernel_image.to_string_lossy().into_owned(),
            initramfs_path: None,
        };
        kernel::install_kernel_binaries(&kernel_artifacts, &target.mount_point)?;

        // 7. Initramfs Generation (WIRED: Fixes Boot Failure)
        info!("Step 7: Generating initramfs via chroot...");
        // Note: If your base rootfs uses mkinitcpio instead of dracut, change this command!
        utils::run_chroot_command(
            &target.mount_point, 
            "dracut --force", 
            None
        )?;

        // 8. System Configuration
        info!("Step 8: Configuring init system...");
        init::configure_init(&target.mount_point, "/usr/lib/systemd/systemd")?;

        info!("Step 9: Installing Limine bootloader...");
        bootloader::install_limine(&efi_mount, &target.root_partition, "bzImage")?;

        // Dual-Boot Detection (WIRED: Adds Windows to Limine if found)
        let limine_conf = efi_mount.join("EFI/BOOT/limine.conf");
        bootloader::detect_and_add_windows(&efi_mount, &limine_conf)?;

        info!("Step 10: Generating system configuration (/etc/fstab, hostname)...");
        config::configure_system(
            &target.mount_point,
            &target.root_partition,
            &target.efi_partition,
            &self.ctx.sys_config.hostname,
            self.ctx.fs_type, // WIRED: Passes Ext4 or Btrfs to generate correct fstab
        )?;

        info!("Step 11: Configuring locale and timezone...");
        locale::configure_locale(
            &target.mount_point,
            &self.ctx.sys_config.locale,
            &self.ctx.sys_config.timezone,
        )?;

        info!("Step 12: Configuring networking...");
        network::configure_network(&target.mount_point)?;

        info!("Step 13: Creating user accounts...");
        users::configure_users(
            &target.mount_point,
            &self.ctx.sys_config.username,
            &self.ctx.sys_config.password_hash,
            &self.ctx.sys_config.password_hash, 
        )?;

        info!("Step 14: Applying security policies..."); 
        security::apply_security_policies(&target.mount_point)?;

        // 9. Hardware Profiling (WIRED: Auto-enables drivers based on hardware)
        info!("Step 15: Profiling hardware for driver injection...");
        let manifest = hardware::profile_hardware();
        if manifest.has_nvidia {
            info!("NVIDIA GPU detected. Enabling persistence daemon...");
            utils::run_chroot_command(&target.mount_point, "systemctl enable nvidia-persistenced", None).ok();
        }
        if manifest.is_laptop {
            info!("Laptop detected. Enabling power management...");
            utils::run_chroot_command(&target.mount_point, "systemctl enable power-profiles-daemon", None).ok();
            utils::run_chroot_command(&target.mount_point, "systemctl enable upower", None).ok();
        }

        // 10. Out-of-Box Experience (OOBE) Handoff
        info!("Step 16: Setting up first-boot OOBE flag...");
        std::fs::write(target.mount_point.join("etc/.mitos-needs-setup"), "1")
            .map_err(|e| format!("Failed to write OOBE flag: {}", e))?;

        info!("Installation pipeline completed successfully!");

        // mount_guard goes out of scope here and automatically safely unmounts everything
        Ok(())
    }
}
