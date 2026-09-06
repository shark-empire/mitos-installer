use dialoguer::{theme::ColorfulTheme, Confirm, Input, Password, Select};
use std::path::PathBuf;

use crate::disk::get_available_disks;
use crate::installer::{InstallationContext, TargetDisk};
use crate::mount::DEFAULT_TARGET_MOUNT;

// 1. Define the stages of our installer wizard
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum SetupStage {
    Welcome,
    Network,
    DiskSelection,
    PartitioningScheme,
    UserConfig,
    Regional,
    Summary,
}

// 2. Define how the user can navigate between stages
#[derive(PartialEq, Clone, Copy)]
pub enum NavAction {
    Next,
    Back,
    Cancel,
}

pub fn run_interactive_setup(ctx: &mut InstallationContext) -> Result<(), String> {
    let mut stage = SetupStage::Welcome;
    let theme = ColorfulTheme::default();

    println!("========================================");
    println!("        MITOS OS Setup Wizard           ");
    println!("========================================\n");

    // 3. The State Machine Loop
    loop {
        match stage {
            SetupStage::Welcome => {
                println!("Welcome to MITOS! This wizard will guide you through the installation process.");
                Confirm::with_theme(&theme)
                    .with_prompt("Press Enter to continue...")
                    .default(true)
                    .interact()
                    .map_err(|e| e.to_string())?;
                stage = SetupStage::Network;
            }
            SetupStage::Network => {
                println!("\n--- Network Configuration ---");
                println!("(Network setup will be wired here using nmcli/iwd in the future)");
                stage = SetupStage::DiskSelection;
            }
            SetupStage::DiskSelection => {
                match select_disk(ctx, &theme)? {
                    NavAction::Next => stage = SetupStage::PartitioningScheme,
                    NavAction::Back => stage = SetupStage::Welcome,
                    NavAction::Cancel => return Err("Installation aborted.".to_string()),
                }
            }
            SetupStage::PartitioningScheme => {
                println!("\n--- Partitioning Scheme ---");
                println!("Selected: Erase disk and install MITOS (Default).");
                // TODO: Wire up Btrfs subvolumes vs Ext4, or Manual Partitioning here
                stage = SetupStage::UserConfig;
            }
            SetupStage::UserConfig => {
                match configure_user(ctx, &theme)? {
                    NavAction::Next => stage = SetupStage::Regional,
                    NavAction::Back => stage = SetupStage::PartitioningScheme,
                    NavAction::Cancel => return Err("Installation aborted.".to_string()),
                }
            }
            SetupStage::Regional => {
                match configure_regional(ctx, &theme)? {
                    NavAction::Next => stage = SetupStage::Summary,
                    NavAction::Back => stage = SetupStage::UserConfig,
                    NavAction::Cancel => return Err("Installation aborted.".to_string()),
                }
            }
            SetupStage::Summary => {
                match show_summary_and_confirm(ctx, &theme)? {
                    NavAction::Next => break, // Breaks the loop, returning Ok(()) to main.rs
                    NavAction::Back => stage = SetupStage::Regional,
                    NavAction::Cancel => return Err("Installation aborted.".to_string()),
                }
            }
        }
    }
    
    Ok(())
}

// ==========================================
// Helper Functions (The actual UI logic)
// ==========================================

fn select_disk(ctx: &mut InstallationContext, theme: &ColorfulTheme) -> Result<NavAction, String> {
    let disks = get_available_disks()?;
    if disks.is_empty() {
        return Err("No writable disks found on this system.".to_string());
    }

    let mut disk_displays: Vec<String> = disks
        .iter()
        .map(|d| {
            let size_gb = d.size / (1024 * 1024 * 1024);
            let model = d.model.as_deref().unwrap_or("Unknown Device");
            format!("{} - {} ({} GB)", d.name, model, size_gb)
        })
        .collect();
        
    // Add a "Go Back" option to the selection menu
    disk_displays.push("< Go Back".to_string());

    let disk_idx = Select::with_theme(theme)
        .with_prompt("Select target disk for MITOS installation")
        .default(0)
        .items(&disk_displays)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    if disk_idx == disks.len() {
        return Ok(NavAction::Back);
    }

    let selected_disk = &disks[disk_idx];

    ctx.target = Some(TargetDisk {
        device_path: selected_disk.path.clone(),
        efi_partition: PathBuf::new(), // Populated later by the pipeline
        root_partition: PathBuf::new(), // Populated later by the pipeline
        mount_point: PathBuf::from(DEFAULT_TARGET_MOUNT),
    });

    Ok(NavAction::Next)
}

fn configure_user(ctx: &mut InstallationContext, theme: &ColorfulTheme) -> Result<NavAction, String> {
    println!("\n--- System Configuration ---");

    ctx.sys_config.hostname = Input::with_theme(theme)
        .with_prompt("System Hostname")
        .default("mitos".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    ctx.sys_config.username = Input::with_theme(theme)
        .with_prompt("Admin Username")
        .default("admin".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    ctx.sys_config.password_hash = Password::with_theme(theme)
        .with_prompt("Admin Password")
        .with_confirmation("Confirm Password", "Passwords do not match")
        .interact()
        .map_err(|e| e.to_string())?;

    Ok(NavAction::Next)
}

fn configure_regional(ctx: &mut InstallationContext, theme: &ColorfulTheme) -> Result<NavAction, String> {
    println!("\n--- Regional Settings ---");

    ctx.sys_config.timezone = Input::with_theme(theme)
        .with_prompt("Timezone (e.g., Africa/Accra)")
        .default("Africa/Accra".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    ctx.sys_config.locale = Input::with_theme(theme)
        .with_prompt("System Locale (e.g., en_US.UTF-8)")
        .default("en_US.UTF-8".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    Ok(NavAction::Next)
}

fn show_summary_and_confirm(ctx: &mut InstallationContext, theme: &ColorfulTheme) -> Result<NavAction, String> {
    let target_path = ctx.target.as_ref().unwrap().device_path.display();
    
    println!("\n========================================");
    println!("           Installation Summary         ");
    println!("========================================");
    println!("Target Disk : {}", target_path);
    println!("Hostname    : {}", ctx.sys_config.hostname);
    println!("Username    : {}", ctx.sys_config.username);
    println!("Timezone    : {}", ctx.sys_config.timezone);
    println!("Locale      : {}", ctx.sys_config.locale);
    println!("========================================\n");

    println!(
        "\nWARNING: All data on {} will be irrevocably destroyed.",
        target_path
    );

    // Instead of a simple Yes/No, we give them explicit navigation choices
    let choices = vec!["Proceed with Installation", "Go Back", "Cancel Installation"];
    let choice = Select::with_theme(theme)
        .with_prompt("Are you absolutely sure you want to proceed?")
        .default(0)
        .items(&choices)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    match choice {
        0 => Ok(NavAction::Next),
        1 => Ok(NavAction::Back),
        _ => Ok(NavAction::Cancel),
    }
}
