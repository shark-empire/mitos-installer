#[allow(deadcode)]
use dialoguer::{theme::ColorfulTheme, Confirm Input, Password, Select};
use std::path::PathBuf;
use std::process::Command;

use crate::config::FilesystemType;
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
                println!("Welcome to MITOS! This wizard will guide you through the installation.");
                println!("You can go back at any stage to change your selections.\n");
                stage = SetupStage::Network;
            }
            SetupStage::Network => match configure_network_ui(&theme)? {
                NavAction::Next => stage = SetupStage::DiskSelection,
                NavAction::Back => stage = SetupStage::Welcome,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::DiskSelection => match select_disk(ctx, &theme)? {
                NavAction::Next => stage = SetupStage::PartitioningScheme,
                NavAction::Back => stage = SetupStage::Network,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::PartitioningScheme => match select_partitioning_scheme(ctx, &theme)? {
                NavAction::Next => stage = SetupStage::UserConfig,
                NavAction::Back => stage = SetupStage::DiskSelection,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::UserConfig => match configure_user(ctx, &theme)? {
                NavAction::Next => stage = SetupStage::Regional,
                NavAction::Back => stage = SetupStage::PartitioningScheme,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::Regional => match configure_regional(ctx, &theme)? {
                NavAction::Next => stage = SetupStage::Summary,
                NavAction::Back => stage = SetupStage::UserConfig,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::Summary => match show_summary_and_confirm(ctx, &theme)? {
                NavAction::Next => break,
                NavAction::Back => stage = SetupStage::Regional,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
        }
    }

    Ok(())
}

// ==========================================
// Helper Functions (The actual UI logic)
// ==========================================

fn configure_network_ui(theme: &ColorfulTheme) -> Result<NavAction, String> {
    println!("\n--- Network Configuration ---");

    let choices = vec![
        "Skip (use current network connection)",
        "Connect to Wi-Fi",
        "< Go Back",
    ];

    let choice = Select::with_theme(theme)
        .with_prompt("Network Setup")
        .default(0)
        .items(&choices)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    match choice {
        0 => Ok(NavAction::Next),
        1 => {
            // Attempt Wi-Fi connection using nmcli
            attempt_wifi_connection(theme)?;
            Ok(NavAction::Next)
        }
        _ => Ok(NavAction::Back),
    }
}

fn attempt_wifi_connection(theme: &ColorfulTheme) -> Result<(), String> {
    // Scan for available Wi-Fi networks using nmcli
    let output = Command::new("nmcli")
        .args(["-t", "-f", "SSID,SIGNAL", "device", "wifi", "list"])
        .output()
        .map_err(|e| format!("Failed to scan Wi-Fi networks: {}", e))?;

    if !output.status.success() {
        println!("Wi-Fi scanning failed. Continuing without Wi-Fi.");
        return Ok(());
    }

    let networks_raw = String::from_utf8_lossy(&output.stdout);
    let networks: Vec<String> = networks_raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    if networks.is_empty() {
        println!("No Wi-Fi networks found. Continuing without Wi-Fi.");
        return Ok(());
    }

    let mut display_names: Vec<String> = networks
        .iter()
        .map(|n| {
            let parts: Vec<&str> = n.splitn(2, ':').collect();
            let ssid = parts.first().unwrap_or(&"Unknown");
            let signal = parts.get(1).unwrap_or(&"?");
            format!("{} (Signal: {}%)", ssid, signal)
        })
        .collect();
    display_names.push("Cancel".to_string());

    let net_idx = Select::with_theme(theme)
        .with_prompt("Select a Wi-Fi network")
        .default(0)
        .items(&display_names)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    if net_idx == networks.len() {
        return Ok(()); // User cancelled
    }

    let ssid: String = networks[net_idx]
        .splitn(2, ':')
        .next()
        .unwrap_or("")
        .to_string();

    let password: String = Password::with_theme(theme)
        .with_prompt(format!("Password for '{}'", ssid))
        .interact()
        .map_err(|e| e.to_string())?;

    println!("Connecting to '{}'...", ssid);
    let status = Command::new("nmcli")
        .args(["device", "wifi", "connect", &ssid, "password", &password])
        .status()
        .map_err(|e| format!("Failed to connect: {}", e))?;

    if status.success() {
        println!("Successfully connected to '{}'!", ssid);
    } else {
        println!("Failed to connect to '{}'. Continuing anyway.", ssid);
    }

    Ok(())
}

fn select_disk(ctx: &mut InstallationContext, theme: &ColorfulTheme) -> Result<NavAction, String> {
    println!("\n--- Disk Selection ---");

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
        efi_partition: PathBuf::new(),
        root_partition: PathBuf::new(),
        mount_point: PathBuf::from(DEFAULT_TARGET_MOUNT),
    });

    Ok(NavAction::Next)
}

fn select_partitioning_scheme(
    ctx: &mut InstallationContext,
    theme: &ColorfulTheme,
) -> Result<NavAction, String> {
    println!("\n--- Partitioning Scheme ---");

    let choices = vec![
        "Ext4  - Traditional, stable, and widely supported",
        "Btrfs - Modern with snapshots, compression, and rollback support",
        "< Go Back",
    ];

    let choice = Select::with_theme(theme)
        .with_prompt("Select root filesystem type")
        .default(0)
        .items(&choices)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    match choice {
        0 => {
            ctx.fs_type = FilesystemType::Ext4;
            Ok(NavAction::Next)
        }
        1 => {
            ctx.fs_type = FilesystemType::Btrfs;
            Ok(NavAction::Next)
        }
        _ => Ok(NavAction::Back),
    }
}

fn configure_user(
    ctx: &mut InstallationContext,
    theme: &ColorfulTheme,
) -> Result<NavAction, String> {
    println!("\n--- System Configuration ---");

    // Hostname input with validation
    ctx.sys_config.hostname = Input::with_theme(theme)
        .with_prompt("System Hostname")
        .default("mitos".to_string())
        .validate_with(|input: &String| -> Result<(), String> {
            let clean: String = input
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if clean.is_empty() {
                Err("Hostname must contain at least one alphanumeric character.".to_string())
            } else if clean.len() > 63 {
                Err("Hostname must not exceed 63 characters.".to_string())
            } else {
                Ok(())
            }
        })
        .interact_text()
        .map_err(|e| e.to_string())?;

    // Username input with validation
    ctx.sys_config.username = Input::with_theme(theme)
        .with_prompt("Admin Username")
        .default("admin".to_string())
        .validate_with(|input: &String| -> Result<(), String> {
            let name = input.trim();
            if name.is_empty() {
                return Err("Username cannot be empty.".to_string());
            }
            if name == "root" {
                return Err("Cannot use 'root' as admin username.".to_string());
            }
            if !name.chars().next().unwrap().is_ascii_lowercase() {
                return Err("Username must start with a lowercase letter.".to_string());
            }
            if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
                return Err("Username can only contain lowercase letters, digits, hyphens, and underscores.".to_string());
            }
            if name.len() > 32 {
                return Err("Username must not exceed 32 characters.".to_string());
            }
            Ok(())
        })
        .interact_text()
        .map_err(|e| e.to_string())?;

    // Password input with strength check
    let raw_password = Password::with_theme(theme)
        .with_prompt("Admin Password")
        .with_confirmation("Confirm Password", "Passwords do not match")
        .validate_with(|input: &String| -> Result<(), String> {
            if input.len() < 4 {
                return Err("Password must be at least 4 characters.".to_string());
            }
            Ok(())
        })
        .interact()
        .map_err(|e| e.to_string())?;

    // SECURITY FIX: Hash the password immediately using SHA-512
    // This prevents plaintext passwords from sitting in memory or leaking into logs
    ctx.sys_config.password_hash = hash_password(&raw_password)?;

    Ok(NavAction::Next)
}

fn configure_regional(
    ctx: &mut InstallationContext,
    theme: &ColorfulTheme,
) -> Result<NavAction, String> {
    println!("\n--- Regional Settings ---");

    // Provide common timezone selections instead of requiring exact string input
    let common_timezones = vec![
        "Africa/Accra",
        "Africa/Lagos",
        "Africa/Nairobi",
        "Africa/Cairo",
        "America/New_York",
        "America/Chicago",
        "America/Denver",
        "America/Los_Angeles",
        "Europe/London",
        "Europe/Berlin",
        "Europe/Paris",
        "Asia/Tokyo",
        "Asia/Shanghai",
        "Asia/Kolkata",
        "Australia/Sydney",
        "Other (type manually)",
        "< Go Back",
    ];

    let tz_idx = Select::with_theme(theme)
        .with_prompt("Select Timezone")
        .default(0)
        .items(&common_timezones)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    // Handle "Go Back"
    if tz_idx == common_timezones.len() - 1 {
        return Ok(NavAction::Back);
    }

    // Handle "Other" (manual input)
    if tz_idx == common_timezones.len() - 2 {
        ctx.sys_config.timezone = Input::with_theme(theme)
            .with_prompt("Enter Timezone (e.g., Pacific/Auckland)")
            .interact_text()
            .map_err(|e| e.to_string())?;
    } else {
        ctx.sys_config.timezone = common_timezones[tz_idx].to_string();
    }

    // Locale selection
    let common_locales = vec![
        "en_US.UTF-8",
        "en_GB.UTF-8",
        "fr_FR.UTF-8",
        "de_DE.UTF-8",
        "es_ES.UTF-8",
        "pt_BR.UTF-8",
        "zh_CN.UTF-8",
        "ja_JP.UTF-8",
        "ar_EG.UTF-8",
        "Other (type manually)",
    ];

    let loc_idx = Select::with_theme(theme)
        .with_prompt("Select System Locale")
        .default(0)
        .items(&common_locales)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    if loc_idx == common_locales.len() - 1 {
        ctx.sys_config.locale = Input::with_theme(theme)
            .with_prompt("Enter Locale (e.g., ko_KR.UTF-8)")
            .interact_text()
            .map_err(|e| e.to_string())?;
    } else {
        ctx.sys_config.locale = common_locales[loc_idx].to_string();
    }

    Ok(NavAction::Next)
}

fn show_summary_and_confirm(
    ctx: &InstallationContext,
    theme: &ColorfulTheme,
) -> Result<NavAction, String> {
    // Safe access: no .unwrap() that could panic
    let target = ctx
        .target
        .as_ref()
        .ok_or("Target disk was not configured.")?;
    let target_path = target.device_path.display();

    let fs_label = match ctx.fs_type {
        FilesystemType::Ext4 => "Ext4",
        FilesystemType::Btrfs => "Btrfs (with subvolumes)",
    };

    println!("\n========================================");
    println!("         Installation Summary           ");
    println!("========================================");
    println!("  Target Disk  : {}", target_path);
    println!("  Filesystem   : {}", fs_label);
    println!("  Hostname     : {}", ctx.sys_config.hostname);
    println!("  Username     : {}", ctx.sys_config.username);
    println!("  Timezone     : {}", ctx.sys_config.timezone);
    println!("  Locale       : {}", ctx.sys_config.locale);
    println!("========================================\n");

    println!(
        "WARNING: ALL data on {} will be irrevocably destroyed.",
        target_path
    );

    let choices = vec![
        "Proceed with Installation",
        "Go Back and Edit",
        "Cancel Installation",
    ];
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

/// Hashes a plaintext password using SHA-512 via openssl.
/// This is what `useradd` and `chpasswd` expect.
fn hash_password(password: &str) -> Result<String, String> {
    let output = Command::new("openssl")
        .args(["passwd", "-6", "-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                stdin.write_all(password.as_bytes()).ok();
            }
            child.wait_with_output()
        })
        .map_err(|e| format!("Failed to hash password: {}", e))?;

    if !output.status.success() {
        return Err(
            "openssl passwd failed. Is openssl installed in the live environment?".to_string(),
        );
    }

    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() {
        return Err("Password hashing produced empty output.".to_string());
    }

    Ok(hash)
}
