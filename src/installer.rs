use std::path::PathBuf;

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
    ctx: InstallationContext,
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
        // Step execution pipeline sequence
        Ok(())
    }
}
