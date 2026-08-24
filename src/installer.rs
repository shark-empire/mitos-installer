use log::{info, error};
use std::path::{Path, PathBuf};

// Assuming all the modules we built are imported
use crate::{
    verify, partition, filesystem, mount::MountGuard,
    rootfs, kernel, init, bootloader, config, users,
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
            },
        }
    }

    pub fn execute(&mut self) -> Result<(), String> {
        info!("Starting MITOS Installation Pipeline...");

        // 1. Validate Context
        // The UI must populate the target disk before calling execute()
        let target = self.ctx.target.as_mut().ok_or("Target disk not configured in context.")?;

        // 2. Verification
        info!("Step 1: Verifying system prerequisites...");
        verify::check_prerequisites()?;
        self.ctx.is_uefi = true; // If verify passes, we are definitely on UEFI

        // 3. Partitioning
        info!("Step 2: Partitioning disk {:?}...", target.device_path);
        let layout = partition::partition_target_disk(&target.device_path)?;
        
        // Update our context with the newly created partition paths
        target.efi_partition = layout.efi_partition.clone();
        target.root_partition = layout.root_partition.clone();

        // 4. Formatting
        info!("Step 3: Formatting partitions...");
        filesystem::format_efi_partition(&target.efi_partition)?;
        filesystem::format_root_partition(&target.root_partition)?;

        // 5. Mounting
        info!("Step 4: Mounting filesystems to {:?}...", target.mount_point);
        let mut mount_guard = MountGuard::new(&target.mount_point);
        mount_guard.mount_target(&target.root_partition, &target.efi_partition)?;

        // 6. Payload Deployment
        // Note: You'll need to define where the installer finds the OS files on the live USB
        let rootfs_archive = Path::new("/run/mitos-live/rootfs.tar"); 
        let kernel_image = Path::new("/run/mitos-live/bzImage");

        info!("Step 5: Unpacking root filesystem...");
        rootfs::unpack_rootfs(rootfs_archive, &target.mount_point)?;

        info!("Step 6: Deploying MITOS kernel...");
        let efi_mount = target.mount_point.join("boot/efi");
        kernel::deploy_kernel(kernel_image, &efi_mount, "bzImage")?;

        // 7. System Configuration
        info!("Step 7: Configuring init system...");
        init::configure_init(&target.mount_point, "/usr/lib/systemd/systemd")?;

        info!("Step 8: Installing Limine bootloader...");
        bootloader::install_limine(&efi_mount, &target.root_partition, "bzImage")?;

        info!("Step 9: Generating system configuration (/etc/fstab, hostname)...");
        config::configure_system(
            &target.mount_point, 
            &target.root_partition, 
            &target.efi_partition, 
            &self.ctx.sys_config.hostname
        )?;

        info!("Step 10: Creating user accounts...");
        users::configure_users(
            &target.mount_point, 
            &self.ctx.sys_config.username, 
            &self.ctx.sys_config.password_hash, 
            &self.ctx.sys_config.password_hash // Using same for root for now
        )?;

        // Note: You can add locale and network configuration modules here next

        info!("Installation pipeline completed successfully!");
        
        // mount_guard goes out of scope here and automatically safely unmounts everything
        Ok(())
    }
}
