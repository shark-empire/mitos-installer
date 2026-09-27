use log::info;
use std::path::{Path, PathBuf};

use crate::logging::TransactionLog;
use crate::{
    bootloader, config, encryption, filesystem, hardware, init, kernel, locale,
    mount::{self, MountGuard},
    network, partition, platform, profile, rootfs, security, users, utils, verify,
};

// -----------------------------------------------------------------------------------
// Progress reporting
// -----------------------------------------------------------------------------------

/// Abstracts over how installation progress is shown to the person: a rich spinner in
/// graphical mode, plain status lines in text/unattended mode. Defined here (rather than
/// tied to a specific UI crate) so `execute()` stays decoupled from exactly how progress
/// is rendered; concrete implementations live in `main.rs`, which is what decides which
/// frontend is active.
pub trait ProgressReporter {
    fn step(&mut self, message: &str);
    fn finish(&mut self, message: &str);
    fn fail(&mut self, message: &str);
}

/// A reporter that does nothing, for contexts (like tests) that don't need progress UX.
pub struct SilentProgress;
impl ProgressReporter for SilentProgress {
    fn step(&mut self, _message: &str) {}
    fn finish(&mut self, _message: &str) {}
    fn fail(&mut self, _message: &str) {}
}

fn announce(progress: &mut dyn ProgressReporter, txn_log: &TransactionLog, msg: &str) {
    info!("{}", msg);
    progress.step(msg);
    txn_log.record(msg);
}

// -----------------------------------------------------------------------------------
// Context types
// -----------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SystemConfig {
    pub hostname: String,
    pub username: String,
    pub password_hash: String,
    pub timezone: String,
    pub locale: String,
    pub keyboard_layout: String,
}

/// How much (if any) swap space to create. `Auto` is resolved to a concrete `Sized(mib)`
/// as early as possible (as soon as RAM size is known) - by the time formatting happens,
/// only `None`/`Sized` should remain, but `execute()` also resolves `Auto` defensively in
/// case a caller didn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwapChoice {
    #[default]
    None,
    Auto,
    Sized(u64), // MiB
}

#[derive(Debug, Clone, Default)]
pub struct EncryptionConfig {
    pub enabled: bool,
    /// Cleared (zeroed) via `utils::secure_zero` as soon as it's no longer needed.
    pub passphrase: String,
}

#[derive(Debug, Clone)]
pub struct TargetDisk {
    pub device_path: PathBuf, // e.g., /dev/nvme0n1
    /// Small unformatted GPT partition for Limine's BIOS stage 2. Only set when
    /// installing under legacy BIOS.
    pub bios_partition: Option<PathBuf>,
    pub efi_partition: PathBuf,
    pub swap_partition: Option<PathBuf>,
    /// Always the *raw* partition, even when encryption is enabled - the opened LUKS
    /// mapper is tracked separately as a local variable within `execute()`, never stored
    /// here, so this field's meaning never changes mid-pipeline.
    pub root_partition: PathBuf,
    pub mount_point: PathBuf, // e.g., /mnt/mitos
}

impl TargetDisk {
    /// Constructs a target disk before partitioning has happened yet: only the disk
    /// itself and where we'll mount it are known. The partition fields are filled in by
    /// `execute()` right after partitioning succeeds.
    pub fn new(device_path: PathBuf, mount_point: PathBuf) -> Self {
        Self {
            device_path,
            bios_partition: None,
            efi_partition: PathBuf::new(),
            swap_partition: None,
            root_partition: PathBuf::new(),
            mount_point,
        }
    }
}

#[derive(Debug)]
pub struct InstallationContext {
    pub target: Option<TargetDisk>,
    pub sys_config: SystemConfig,
    pub is_uefi: bool,
    pub fs_type: config::FilesystemType,
    pub swap: SwapChoice,
    pub encryption: EncryptionConfig,
    pub install_profile: profile::InstallProfile,
    pub desktop_choice: profile::DesktopChoice,
    /// Installer UI language, e.g. "en-US" - only meaningful to the UI layer, carried
    /// here so unattended mode can set it too and have it apply consistently.
    pub ui_language: String,
    /// (SSID, passphrase) captured during live-environment Wi-Fi setup, carried over to
    /// the installed system's network config if present.
    pub wifi_credentials: Option<(String, String)>,
}

pub struct InstallerPipeline {
    pub ctx: InstallationContext, // Made public so the UI can modify it before execution
}

impl Default for InstallerPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl InstallerPipeline {
    pub fn new() -> Self {
        Self {
            ctx: InstallationContext {
                target: None,
                sys_config: SystemConfig::default(),
                is_uefi: false,
                fs_type: config::FilesystemType::Ext4,
                swap: SwapChoice::None,
                encryption: EncryptionConfig::default(),
                install_profile: profile::InstallProfile::default(),
                desktop_choice: profile::DesktopChoice::default(),
                ui_language: "en-US".to_string(),
                wifi_credentials: None,
            },
        }
    }

    /// Returns the info `recovery::trigger_emergency_cleanup` needs, based on however far
    /// the context got filled in before a failure. Safe to call at any point: fields that
    /// were never reached are simply `None`.
    pub fn cleanup_info(&self) -> (PathBuf, Option<PathBuf>, Option<PathBuf>, Option<&'static str>) {
        let target_mount = self
            .ctx
            .target
            .as_ref()
            .map(|t| t.mount_point.clone())
            .unwrap_or_else(|| PathBuf::from(mount::DEFAULT_TARGET_MOUNT));
        let target_disk = self.ctx.target.as_ref().map(|t| t.device_path.clone());
        let swap_partition = self.ctx.target.as_ref().and_then(|t| t.swap_partition.clone());
        let luks_mapper = if self.ctx.encryption.enabled {
            Some(encryption::ROOT_MAPPER_NAME)
        } else {
            None
        };
        (target_mount, target_disk, swap_partition, luks_mapper)
    }

    pub fn execute(
        &mut self,
        txn_log: &TransactionLog,
        progress: &mut dyn ProgressReporter,
    ) -> Result<(), String> {
        info!("Starting MITOS Installation Pipeline...");
        txn_log.record("installation pipeline started");

        let device_path = self
            .ctx
            .target
            .as_ref()
            .ok_or("Target disk not configured in context.")?
            .device_path
            .clone();
        let mount_point = self.ctx.target.as_ref().unwrap().mount_point.clone();

        // --- Step: prerequisites -------------------------------------------------
        announce(progress, txn_log, "Verifying system prerequisites...");
        let platform_info = platform::detect_platform();
        self.ctx.is_uefi = platform_info.boot_mode == platform::BootMode::Uefi;

        let manifest = hardware::profile_hardware();
        hardware::check_minimum_requirements(&manifest)?;

        // Pin down swap to a concrete size now that we know how much RAM is installed.
        if self.ctx.swap == SwapChoice::Auto {
            self.ctx.swap = SwapChoice::Sized(hardware::recommended_swap_mib(manifest.ram_total_mib));
        }
        let swap_mib: Option<u64> = match self.ctx.swap {
            SwapChoice::None => None,
            SwapChoice::Sized(n) => Some(n),
            SwapChoice::Auto => unreachable!("resolved above"),
        };

        let prereq_opts = verify::PrerequisiteOptions {
            uses_btrfs: self.ctx.fs_type == config::FilesystemType::Btrfs,
            is_uefi: self.ctx.is_uefi,
            wants_encryption: self.ctx.encryption.enabled,
            wants_swap: swap_mib.is_some(),
        };
        verify::check_prerequisites(&prereq_opts)?;

        // --- Step: capacity validation --------------------------------------------
        announce(progress, txn_log, "Validating target disk capacity...");
        let part_opts = partition::PartitionOptions {
            bios_boot: !self.ctx.is_uefi,
            swap_mib,
        };
        let disk_size = partition::disk_size_bytes(&device_path)?;
        partition::validate_disk_capacity(disk_size, &part_opts)?;

        // --- Step: partitioning ----------------------------------------------------
        announce(progress, txn_log, &format!("Partitioning disk {:?}...", device_path));
        let layout = partition::partition_target_disk(&device_path, &part_opts)?;

        {
            let target = self.ctx.target.as_mut().unwrap();
            target.bios_partition = layout.bios_partition.clone();
            target.efi_partition = layout.efi_partition.clone();
            target.swap_partition = layout.swap_partition.clone();
            target.root_partition = layout.root_partition.clone();
        }
        let raw_root_partition = layout.root_partition.clone();

        // --- Step: encryption (must happen before formatting) ----------------------
        let mut effective_root = raw_root_partition.clone();
        let mut luks_boot_info: Option<bootloader::LuksBootInfo> = None;
        if self.ctx.encryption.enabled {
            announce(progress, txn_log, "Setting up disk encryption (LUKS2)...");
            let mapper = encryption::setup_luks(&raw_root_partition, &self.ctx.encryption.passphrase)?;
            let raw_uuid = encryption::luks_uuid(&raw_root_partition)?;
            utils::secure_zero(&mut self.ctx.encryption.passphrase);

            luks_boot_info = Some(bootloader::LuksBootInfo {
                raw_partition_uuid: raw_uuid,
                mapper_name: encryption::ROOT_MAPPER_NAME.to_string(),
            });
            effective_root = mapper;
        }

        // --- Step: formatting --------------------------------------------------------
        announce(progress, txn_log, "Formatting partitions...");
        filesystem::format_efi_partition(&layout.efi_partition)?;

        let is_btrfs = self.ctx.fs_type == config::FilesystemType::Btrfs;
        if is_btrfs {
            info!("Creating Btrfs subvolume layout...");
            filesystem::create_btrfs_layout(&effective_root)?;
        } else {
            filesystem::format_root_partition(&effective_root, self.ctx.fs_type)?;
        }

        if let Some(swap_partition) = &layout.swap_partition {
            announce(progress, txn_log, "Formatting and enabling swap...");
            filesystem::format_and_enable_swap(swap_partition)?;
        }

        // --- Step: mounting ------------------------------------------------------------
        announce(progress, txn_log, &format!("Mounting filesystems to {:?}...", mount_point));
        let mut mount_guard = MountGuard::new(&mount_point);
        mount_guard.mount_root(&effective_root, is_btrfs)?;

        if is_btrfs {
            info!("Mounting Btrfs subvolumes...");
            mount_guard.mount_btrfs_subvolume(&effective_root, "@home", "home")?;
            mount_guard.mount_btrfs_subvolume(&effective_root, "@var", "var")?;
            mount_guard.mount_btrfs_subvolume(&effective_root, "@log", "var/log")?;
        }

        mount_guard.mount_efi(&layout.efi_partition)?;

        // --- Step: rootfs deployment -----------------------------------------------
        let rootfs_archive = Path::new("/run/mitos-live/rootfs.tar");
        let kernel_image = Path::new("/run/mitos-live/bzImage");

        announce(progress, txn_log, "Checking available space for the base system...");
        if let Ok(archive_meta) = std::fs::metadata(rootfs_archive) {
            // Uncompressed tar archives extract to roughly their own size or larger;
            // this is a best-effort sanity check, not an exact accounting.
            partition::check_available_space(&mount_point, archive_meta.len())?;
        }

        announce(progress, txn_log, "Unpacking root filesystem (this may take a while)...");
        let rootfs_source = rootfs::RootfsSource::Archive(rootfs_archive.to_string_lossy().into_owned());
        rootfs::deploy_rootfs(&rootfs_source, &mount_point)?;

        // CRITICAL: Bind pseudo-filesystems BEFORE ANY chroot commands
        announce(progress, txn_log, "Binding pseudo-filesystems for the chroot environment...");
        mount_guard.mount_pseudo_filesystems()?;

        announce(progress, txn_log, "Deploying MITOS kernel...");
        let efi_mount = mount_point.join("boot/efi");
        let kernel_artifacts = kernel::KernelArtifacts {
            kernel_path: kernel_image.to_string_lossy().into_owned(),
            initramfs_path: None,
        };
        kernel::install_kernel_binaries(&kernel_artifacts, &mount_point)?;

        announce(progress, txn_log, "Generating initramfs via chroot...");
        // Note: If your base rootfs uses mkinitcpio instead of dracut, change this command!
        utils::run_chroot_command(&mount_point, "dracut --force", None)?;

        announce(progress, txn_log, "Configuring init system...");
        init::configure_init(&mount_point, "/usr/lib/systemd/systemd")?;

        // --- Step: bootloader --------------------------------------------------------
        announce(progress, txn_log, "Installing the Limine bootloader...");
        let bootloader_req = bootloader::BootloaderRequest {
            disk_path: &device_path,
            esp_mount: &efi_mount,
            esp_partition: &layout.efi_partition,
            bios_stage2_partition: layout.bios_partition.as_deref(),
            root_partition: &raw_root_partition,
            kernel_name: kernel::DEFAULT_KERNEL_NAME,
            initramfs_name: kernel::DEFAULT_INITRAMFS_NAME,
            luks: luks_boot_info.as_ref(),
        };
        if self.ctx.is_uefi {
            bootloader::install_limine_uefi(&bootloader_req)?;
        } else {
            bootloader::install_limine_bios(&bootloader_req)?;
        }
        bootloader::detect_and_add_windows(&efi_mount)?;

        // --- Step: system configuration ----------------------------------------------
        announce(progress, txn_log, "Generating system configuration (fstab, hostname)...");
        let luks_crypttab_info = luks_boot_info
            .as_ref()
            .map(|info| config::LuksCrypttabInfo {
                mapper_name: &info.mapper_name,
                uuid: &info.raw_partition_uuid,
            });
        let sys_config_req = config::SystemConfigRequest {
            target_mount: &mount_point,
            root_fs_device: &effective_root,
            efi_partition: &layout.efi_partition,
            swap_partition: layout.swap_partition.as_deref(),
            luks: luks_crypttab_info,
            hostname: &self.ctx.sys_config.hostname,
            fs_type: self.ctx.fs_type,
        };
        config::configure_system(&sys_config_req)?;

        announce(progress, txn_log, "Configuring locale, timezone, and keyboard...");
        locale::configure_locale(&mount_point, &self.ctx.sys_config.locale, &self.ctx.sys_config.timezone)?;
        locale::configure_keyboard(&mount_point, &self.ctx.sys_config.keyboard_layout)?;

        announce(progress, txn_log, "Configuring networking...");
        network::configure_network(&mount_point, self.ctx.wifi_credentials.as_ref())?;

        announce(progress, txn_log, "Creating user accounts...");
        users::configure_users(
            &mount_point,
            &self.ctx.sys_config.username,
            &self.ctx.sys_config.password_hash,
            &self.ctx.sys_config.password_hash,
        )?;

        announce(progress, txn_log, "Applying security policies...");
        security::apply_security_policies(&mount_point)?;

        // --- Step: software selection --------------------------------------------------
        announce(progress, txn_log, "Applying desktop selection...");
        profile::apply_desktop_choice(self.ctx.desktop_choice, &mount_point)?;

        announce(progress, txn_log, "Installing selected software packages...");
        profile::deploy_profile(self.ctx.install_profile, &mount_point)?;

        // --- Step: hardware-driven service enablement -----------------------------------
        announce(progress, txn_log, "Enabling drivers/services for detected hardware...");
        if manifest.has_nvidia {
            info!("NVIDIA GPU detected. Enabling persistence daemon...");
            utils::run_chroot_command(&mount_point, "systemctl enable nvidia-persistenced", None).ok();
        }
        if manifest.is_laptop {
            info!("Laptop detected. Enabling power management...");
            utils::run_chroot_command(&mount_point, "systemctl enable power-profiles-daemon", None).ok();
            utils::run_chroot_command(&mount_point, "systemctl enable upower", None).ok();
        }

        // --- Step: OOBE flag -------------------------------------------------------------
        announce(progress, txn_log, "Setting up first-boot experience...");
        std::fs::write(mount_point.join("etc/.mitos-needs-setup"), "1")
            .map_err(|e| format!("Failed to write OOBE flag: {}", e))?;

        // Copy the install logs onto the target so recovery mode can read them later, even
        // if the live environment's own /var/log doesn't survive to the next boot.
        let _ = std::fs::copy(
            crate::logging::DEFAULT_LOG_PATH,
            mount_point.join("var/log/mitos-install.log"),
        );
        let _ = std::fs::copy(txn_log.path(), mount_point.join("var/log/mitos-install-transactions.log"));

        // --- Finalize ----------------------------------------------------------------------
        announce(progress, txn_log, "Finalizing and unmounting...");
        mount_guard.unmount_all()?;
        if self.ctx.encryption.enabled {
            if let Err(e) = encryption::close_luks(encryption::ROOT_MAPPER_NAME) {
                log::warn!("Could not close LUKS mapping after a successful install: {}", e);
            }
        }

        txn_log.record("installation pipeline completed successfully");
        progress.finish("Installation complete!");
        info!("Installation pipeline completed successfully!");

        Ok(())
    }
}
