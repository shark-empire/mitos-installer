use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use std::path::PathBuf;

use crate::disk::get_available_disks;
use crate::installer::{InstallationContext, SystemConfig, TargetDisk};
use crate::mount::DEFAULT_TARGET_MOUNT;

pub fn run_interactive_setup(ctx: &mut InstallationContext) -> Result<(), String> {
    let theme = ColorfulTheme::default();

    println!("========================================");
    println!("        MITOS OS Setup Wizard           ");
    println!("========================================\n");

    // 1. Disk Selection
    let disks = get_available_disks()?;
    if disks.is_empty() {
        return Err("No writable disks found on this system.".to_string());
    }

    let disk_displays: Vec<String> = disks
        .iter()
        .map(|d| {
            let size_gb = d.size / (1024 * 1024 * 1024);
            let model = d.model.as_deref().unwrap_or("Unknown Device");
            format!("{} - {} ({} GB)", d.name, model, size_gb)
        })
        .collect();

    let disk_idx = Select::with_theme(&theme)
        .with_prompt("Select target disk for MITOS installation")
        .default(0)
        .items(&disk_displays)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    let selected_disk = &disks[disk_idx];

    ctx.target = Some(TargetDisk {
        device_path: selected_disk.path.clone(),
        efi_partition: PathBuf::new(),  // Populated later by the pipeline
        root_partition: PathBuf::new(), // Populated later by the pipeline
        mount_point: PathBuf::from(DEFAULT_TARGET_MOUNT),
    });

    // 2. System Configuration
    println!("\n--- System Configuration ---");
    
    ctx.sys_config.hostname = Input::with_theme(&theme)
        .with_prompt("System Hostname")
        .default("mitos".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    ctx.sys_config.username = Input::with_theme(&theme)
        .with_prompt("Admin Username")
        .default("admin".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    ctx.sys_config.password_hash = Password::with_theme(&theme)
        .with_prompt("Admin Password")
        .with_confirmation("Confirm Password", "Passwords do not match")
        .interact()
        .map_err(|e| e.to_string())?;

    // 3. Regional Settings
    println!("\n--- Regional Settings ---");

    ctx.sys_config.timezone = Input::with_theme(&theme)
        .with_prompt("Timezone")
        .default("Africa/Accra".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    ctx.sys_config.locale = Input::with_theme(&theme)
        .with_prompt("System Locale")
        .default("en_US.UTF-8".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    // 4. Final Confirmation
    println!("\nWARNING: All data on {} will be irrevocably destroyed.", selected_disk.path.display());
    
    let proceed = Confirm::with_theme(&theme)
        .with_prompt("Are you absolutely sure you want to proceed?")
        .default(false)
        .interact()
        .map_err(|e| e.to_string())?;

    if !proceed {
        return Err("Installation aborted by user.".to_string());
    }

    Ok(())
}
