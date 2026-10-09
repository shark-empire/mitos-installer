use dialoguer::theme::{ColorfulTheme, SimpleTheme, Theme};
use dialoguer::{Confirm, Input, Password, Select};
use rust_i18n::t;
use std::path::PathBuf;
use std::process::Command;

use crate::config::FilesystemType;
use crate::disk::get_available_disks;
use crate::hardware::{self, HardwareManifest};
use crate::installer::{EncryptionConfig, InstallationContext, SwapChoice, TargetDisk};
use crate::logging::TransactionLog;
use crate::mount::DEFAULT_TARGET_MOUNT;
use crate::utils::hash_password;
use crate::{partition, profile, recovery};

// 1. Define the stages of our installer wizard
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum SetupStage {
    Language,
    Welcome,
    Network,
    DiskSelection,
    PartitioningScheme,
    UserConfig,
    Regional,
    Software,
    Summary,
}

// 2. Define how the user can navigate between stages
#[derive(PartialEq, Clone, Copy)]
pub enum NavAction {
    Next,
    Back,
    Cancel,
}

/// Runs the interactive setup wizard and populates `ctx` from the person's answers.
///
/// `graphical` picks which dialoguer theme (and banner style) is used. Both "graphical"
/// and "text" installer modes are this same terminal wizard, just dressed differently -
/// this project's own docs describe it as a TUI installer, and there's no pixel-based GUI
/// toolkit in the dependency tree to build a second, separate frontend from. The text
/// theme exists for serial consoles / low-bandwidth SSH sessions where a plain, colorless,
/// linear prompt is more reliable than styled arrow-key menus.
pub fn run_interactive_setup(ctx: &mut InstallationContext, graphical: bool) -> Result<(), String> {
    let theme: Box<dyn Theme> = if graphical {
        Box::new(ColorfulTheme::default())
    } else {
        Box::new(SimpleTheme)
    };
    let theme = theme.as_ref();

    // Gathered once up front (not re-probed every time a stage is revisited via "Back")
    // and reused both for the swap-size suggestion and the final summary.
    let manifest = hardware::profile_hardware();

    let mut stage = SetupStage::Language;

    if graphical {
        println!("========================================");
        println!("        MITOS OS Setup Wizard           ");
        println!("========================================\n");
    } else {
        println!("MITOS OS Setup Wizard\n");
    }

    // 3. The State Machine Loop
    loop {
        match stage {
            SetupStage::Language => {
                select_language(ctx, theme)?;
                stage = SetupStage::Welcome;
            }
            SetupStage::Welcome => {
                println!("{}", t!("welcome"));
                println!("{}\n", t!("welcome_body"));
                stage = SetupStage::Network;
            }
            SetupStage::Network => match configure_network_ui(ctx, theme)? {
                NavAction::Next => stage = SetupStage::DiskSelection,
                NavAction::Back => stage = SetupStage::Welcome,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::DiskSelection => match select_disk(ctx, theme)? {
                NavAction::Next => stage = SetupStage::PartitioningScheme,
                NavAction::Back => stage = SetupStage::Network,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::PartitioningScheme => match configure_disk_layout(ctx, theme, &manifest)? {
                NavAction::Next => stage = SetupStage::UserConfig,
                NavAction::Back => stage = SetupStage::DiskSelection,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::UserConfig => match configure_user(ctx, theme)? {
                NavAction::Next => stage = SetupStage::Regional,
                NavAction::Back => stage = SetupStage::PartitioningScheme,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::Regional => match configure_regional(ctx, theme)? {
                NavAction::Next => stage = SetupStage::Software,
                NavAction::Back => stage = SetupStage::UserConfig,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::Software => match configure_software(ctx, theme)? {
                NavAction::Next => stage = SetupStage::Summary,
                NavAction::Back => stage = SetupStage::Regional,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
            SetupStage::Summary => match show_summary_and_confirm(ctx, theme)? {
                NavAction::Next => break,
                NavAction::Back => stage = SetupStage::Software,
                NavAction::Cancel => return Err("Installation aborted.".to_string()),
            },
        }
    }

    Ok(())
}

// ==========================================
// Helper Functions (The actual UI logic)
// ==========================================

fn select_language(ctx: &mut InstallationContext, theme: &dyn Theme) -> Result<(), String> {
    let locales: Vec<String> = rust_i18n::available_locales!()
        .iter()
        .map(|s| s.to_string())
        .collect();

    if locales.len() <= 1 {
        // Nothing meaningful to choose between; keep the default and move on quietly.
        return Ok(());
    }

    let default_idx = locales
        .iter()
        .position(|l| l == &ctx.ui_language)
        .unwrap_or(0);

    let idx = Select::with_theme(theme)
        .with_prompt("Select installer language / Selecciona el idioma")
        .default(default_idx)
        .items(&locales)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    ctx.ui_language = locales[idx].clone();
    rust_i18n::set_locale(&ctx.ui_language);
    Ok(())
}

fn configure_network_ui(
    ctx: &mut InstallationContext,
    theme: &dyn Theme,
) -> Result<NavAction, String> {
    println!("\n--- {} ---", t!("network_heading"));

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
            if let Some(credentials) = attempt_wifi_connection(theme)? {
                ctx.wifi_credentials = Some(credentials);
            }
            Ok(NavAction::Next)
        }
        _ => Ok(NavAction::Back),
    }
}

/// Scans for and connects to a Wi-Fi network via `nmcli`. Returns the (SSID, password)
/// pair on a successful connection, so the caller can carry it over into the installed
/// system's own network config later - or `None` if the person skipped/cancelled, or the
/// connection attempt failed (in which case we don't want to carry over a maybe-wrong
/// password anyway).
fn attempt_wifi_connection(theme: &dyn Theme) -> Result<Option<(String, String)>, String> {
    // Scan for available Wi-Fi networks using nmcli
    let output = Command::new("nmcli")
        .args(["-t", "-f", "SSID,SIGNAL", "device", "wifi", "list"])
        .output()
        .map_err(|e| format!("Failed to scan Wi-Fi networks: {}", e))?;

    if !output.status.success() {
        println!("Wi-Fi scanning failed. Continuing without Wi-Fi.");
        return Ok(None);
    }

    let networks_raw = String::from_utf8_lossy(&output.stdout);
    let networks: Vec<String> = networks_raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();

    if networks.is_empty() {
        println!("No Wi-Fi networks found. Continuing without Wi-Fi.");
        return Ok(None);
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
        return Ok(None); // User cancelled
    }

    let ssid: String = networks[net_idx]
        .split(':')
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
        Ok(Some((ssid, password)))
    } else {
        println!("Failed to connect to '{}'. Continuing anyway.", ssid);
        Ok(None)
    }
}

fn select_disk(ctx: &mut InstallationContext, theme: &dyn Theme) -> Result<NavAction, String> {
    println!("\n--- {} ---", t!("disk_heading"));

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

    ctx.target = Some(TargetDisk::new(
        selected_disk.path.clone(),
        PathBuf::from(DEFAULT_TARGET_MOUNT),
    ));

    Ok(NavAction::Next)
}

/// Filesystem type, swap, and encryption - the three "how should the disk be laid out"
/// choices - grouped into a single stage the same way the original code grouped
/// hostname/username/password into one "System Configuration" stage.
fn configure_disk_layout(
    ctx: &mut InstallationContext,
    theme: &dyn Theme,
    manifest: &HardwareManifest,
) -> Result<NavAction, String> {
    println!("\n--- {} ---", t!("partitioning_heading"));

    let fs_choices = vec![
        "Ext4  - Traditional, stable, and widely supported",
        "Btrfs - Modern with snapshots, compression, and rollback support",
        "< Go Back",
    ];

    let default_fs = match ctx.fs_type {
        FilesystemType::Ext4 => 0,
        FilesystemType::Btrfs => 1,
    };

    let fs_choice = Select::with_theme(theme)
        .with_prompt("Select root filesystem type")
        .default(default_fs)
        .items(&fs_choices)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    match fs_choice {
        0 => ctx.fs_type = FilesystemType::Ext4,
        1 => ctx.fs_type = FilesystemType::Btrfs,
        _ => return Ok(NavAction::Back),
    }

    // Swap
    let recommended_mib = hardware::recommended_swap_mib(manifest.ram_total_mib);
    let swap_choices = vec![
        format!("Automatic (recommended: {} MB)", recommended_mib),
        "Custom size".to_string(),
        "No swap".to_string(),
    ];

    let swap_choice = Select::with_theme(theme)
        .with_prompt("Swap space")
        .default(0)
        .items(&swap_choices)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    ctx.swap = match swap_choice {
        0 => SwapChoice::Sized(recommended_mib),
        1 => {
            let mb: u64 = Input::with_theme(theme)
                .with_prompt("Swap size in MB")
                .default(recommended_mib)
                .validate_with(|input: &u64| -> Result<(), String> {
                    if *input == 0 {
                        Err(
                            "Swap size must be greater than 0. Choose 'No swap' instead if \
                             you don't want any."
                                .to_string(),
                        )
                    } else {
                        Ok(())
                    }
                })
                .interact_text()
                .map_err(|e| e.to_string())?;
            SwapChoice::Sized(mb)
        }
        _ => SwapChoice::None,
    };

    // Encryption
    let encrypt = Confirm::with_theme(theme)
        .with_prompt(
            "Encrypt the disk with LUKS2? You'll need to enter this passphrase every time \
             you boot.",
        )
        .default(false)
        .interact()
        .map_err(|e| e.to_string())?;

    if encrypt {
        let passphrase = Password::with_theme(theme)
            .with_prompt("Encryption passphrase")
            .with_confirmation("Confirm passphrase", "Passphrases do not match")
            .validate_with(|input: &String| -> Result<(), String> {
                if input.len() < 8 {
                    Err("Passphrase must be at least 8 characters.".to_string())
                } else {
                    Ok(())
                }
            })
            .interact()
            .map_err(|e| e.to_string())?;

        ctx.encryption = EncryptionConfig {
            enabled: true,
            passphrase,
        };
    } else {
        ctx.encryption = EncryptionConfig::default();
    }

    Ok(NavAction::Next)
}

fn configure_user(ctx: &mut InstallationContext, theme: &dyn Theme) -> Result<NavAction, String> {
    println!("\n--- {} ---", t!("user_heading"));

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
            if !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                return Err(
                    "Username can only contain lowercase letters, digits, hyphens, and \
                     underscores."
                        .to_string(),
                );
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

    // SECURITY: Hash the password immediately using SHA-512, so a plaintext password
    // doesn't sit around in the context struct for the rest of the run.
    ctx.sys_config.password_hash = hash_password(&raw_password)?;

    Ok(NavAction::Next)
}

fn configure_regional(
    ctx: &mut InstallationContext,
    theme: &dyn Theme,
) -> Result<NavAction, String> {
    println!("\n--- {} ---", t!("regional_heading"));

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

    // Keyboard layout selection
    let common_layouts = vec![
        "us - US English",
        "gb - British English",
        "de - German",
        "fr - French",
        "es - Spanish",
        "it - Italian",
        "pt - Portuguese",
        "jp - Japanese",
        "Other (type manually)",
    ];

    let kb_idx = Select::with_theme(theme)
        .with_prompt("Select Keyboard Layout")
        .default(0)
        .items(&common_layouts)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    if kb_idx == common_layouts.len() - 1 {
        ctx.sys_config.keyboard_layout = Input::with_theme(theme)
            .with_prompt("Enter XKB keyboard layout code (e.g., se, no, pl)")
            .default("us".to_string())
            .interact_text()
            .map_err(|e| e.to_string())?;
    } else {
        let code = common_layouts[kb_idx].split(" - ").next().unwrap_or("us");
        ctx.sys_config.keyboard_layout = code.to_string();
    }

    Ok(NavAction::Next)
}

fn configure_software(
    ctx: &mut InstallationContext,
    theme: &dyn Theme,
) -> Result<NavAction, String> {
    println!("\n--- {} ---", t!("software_heading"));

    let mut desktop_labels: Vec<&str> = profile::DesktopChoice::ALL
        .iter()
        .map(|d| d.label())
        .collect();
    desktop_labels.push("< Go Back");

    let desktop_idx = Select::with_theme(theme)
        .with_prompt("Desktop")
        .default(0)
        .items(&desktop_labels)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    if desktop_idx == profile::DesktopChoice::ALL.len() {
        return Ok(NavAction::Back);
    }
    ctx.desktop_choice = profile::DesktopChoice::ALL[desktop_idx];

    let profile_labels: Vec<&str> = profile::InstallProfile::ALL
        .iter()
        .map(|p| p.label())
        .collect();
    let profile_idx = Select::with_theme(theme)
        .with_prompt("Package selection")
        .default(1) // Standard
        .items(&profile_labels)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;
    ctx.install_profile = profile::InstallProfile::ALL[profile_idx];

    Ok(NavAction::Next)
}

fn show_summary_and_confirm(
    ctx: &InstallationContext,
    theme: &dyn Theme,
) -> Result<NavAction, String> {
    // Safe access: no .unwrap() that could panic
    let target = ctx
        .target
        .as_ref()
        .ok_or("Target disk was not configured.")?;
    let target_path = target.device_path.display().to_string();

    let fs_label = match ctx.fs_type {
        FilesystemType::Ext4 => "Ext4",
        FilesystemType::Btrfs => "Btrfs (with subvolumes)",
    };
    let swap_label = match ctx.swap {
        SwapChoice::None => "None".to_string(),
        SwapChoice::Sized(mb) => format!("{} MB", mb),
        SwapChoice::Auto => "Automatic".to_string(),
    };
    let encryption_label = if ctx.encryption.enabled {
        "Enabled (LUKS2)"
    } else {
        "Disabled"
    };

    println!("\n========================================");
    println!("       {}       ", t!("summary_heading"));
    println!("========================================");
    println!("  Target Disk  : {}", target_path);
    println!("  Filesystem   : {}", fs_label);
    println!("  Swap         : {}", swap_label);
    println!("  Encryption   : {}", encryption_label);
    println!("  Hostname     : {}", ctx.sys_config.hostname);
    println!("  Username     : {}", ctx.sys_config.username);
    println!("  Timezone     : {}", ctx.sys_config.timezone);
    println!("  Locale       : {}", ctx.sys_config.locale);
    println!("  Keyboard     : {}", ctx.sys_config.keyboard_layout);
    println!("  Desktop      : {}", ctx.desktop_choice.label());
    println!("  Packages     : {}", ctx.install_profile.label());
    println!("========================================\n");

    // Early, non-authoritative capacity check so a too-small disk is caught here rather
    // than after several other steps have already run; execute() re-checks this too.
    if let Ok(disk_size) = partition::disk_size_bytes(&target.device_path) {
        let swap_mib = match ctx.swap {
            SwapChoice::Sized(mb) => Some(mb),
            _ => None,
        };
        let part_opts = partition::PartitionOptions {
            bios_boot: !ctx.is_uefi,
            swap_mib,
        };
        if let Err(e) = partition::validate_disk_capacity(disk_size, &part_opts) {
            println!("WARNING: {}\n", e);
        }
    }

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
        // Defaults to "Go Back" rather than "Proceed", so an inattentive Enter press
        // can't trigger a destructive, irreversible action.
        .default(1)
        .items(&choices)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    match choice {
        0 => {
            // A single arrow-key selection is a weak confirmation for something this
            // destructive and irreversible - require typing the exact disk path too.
            let typed: String = Input::with_theme(theme)
                .with_prompt(format!("Type the disk path ({}) to confirm", target_path))
                .allow_empty(true)
                .interact_text()
                .map_err(|e| e.to_string())?;

            if typed.trim() == target_path {
                Ok(NavAction::Next)
            } else {
                println!(
                    "\nThat didn't match {}. Returning to the summary.\n",
                    target_path
                );
                Ok(NavAction::Back)
            }
        }
        1 => Ok(NavAction::Back),
        _ => Ok(NavAction::Cancel),
    }
}

// ==========================================
// Recovery mode UI
// ==========================================

/// Runs the recovery-mode menu: find an existing installation, open it, then loop over
/// the five recovery actions until the person chooses to exit.
pub fn run_recovery_menu(graphical: bool, txn_log: &TransactionLog) -> Result<(), String> {
    let theme: Box<dyn Theme> = if graphical {
        Box::new(ColorfulTheme::default())
    } else {
        Box::new(SimpleTheme)
    };
    let theme = theme.as_ref();

    println!("\n========================================");
    println!("      {}      ", t!("recovery_heading"));
    println!("========================================\n");

    let candidates = recovery::list_installation_candidates()?;
    if candidates.is_empty() {
        println!("{}", t!("recovery_no_installs"));
        return Ok(());
    }

    let mut display: Vec<String> = candidates.iter().map(|c| c.description()).collect();
    display.push("< Cancel".to_string());

    let idx = Select::with_theme(theme)
        .with_prompt("Select an installation to recover")
        .default(0)
        .items(&display)
        .interact()
        .map_err(|e| format!("UI error: {}", e))?;

    if idx == candidates.len() {
        return Ok(());
    }
    let candidate = candidates[idx].clone();

    let passphrase: Option<String> = if candidate.is_luks {
        Some(
            Password::with_theme(theme)
                .with_prompt("Encryption passphrase")
                .interact()
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };

    let session = recovery::open_session(&candidate, passphrase.as_deref())?;
    println!(
        "\nMounted {} at {:?}\n",
        candidate.root_partition.display(),
        session.target_mount()
    );

    loop {
        let actions = vec![
            "Repair boot (reinstall bootloader)",
            "Repair packages",
            "Reinstall system components",
            "Inspect logs",
            "Restore configuration",
            "Exit recovery mode",
        ];
        let action_idx = Select::with_theme(theme)
            .with_prompt("Recovery action")
            .default(0)
            .items(&actions)
            .interact()
            .map_err(|e| format!("UI error: {}", e))?;

        match action_idx {
            0 => match recovery::repair_boot(&session, txn_log) {
                Ok(()) => println!("\nBoot repair completed successfully.\n"),
                Err(e) => println!("\nBoot repair failed: {}\n", e),
            },
            1 => match recovery::repair_packages(&session, txn_log) {
                Ok(msg) => println!("\n{}\n", msg),
                Err(e) => println!("\nPackage repair failed: {}\n", e),
            },
            2 => {
                let confirmed = Confirm::with_theme(theme)
                    .with_prompt(
                        "This re-extracts the base system over the existing installation \
                         (files in /home are not affected). Continue?",
                    )
                    .default(false)
                    .interact()
                    .map_err(|e| e.to_string())?;
                if confirmed {
                    match recovery::reinstall_system_components(&session, txn_log) {
                        Ok(()) => println!("\nSystem components reinstalled successfully.\n"),
                        Err(e) => println!("\nReinstall failed: {}\n", e),
                    }
                }
            }
            3 => {
                let report = recovery::inspect_logs(&session, txn_log);
                println!("\n{}\n", report);
            }
            4 => match prompt_restore_config(theme, &session) {
                Ok(input) => match recovery::restore_configuration(&session, &input, txn_log) {
                    Ok(()) => println!("\nConfiguration restored successfully.\n"),
                    Err(e) => println!("\nRestore failed: {}\n", e),
                },
                Err(e) => println!("\n{}\n", e),
            },
            _ => break,
        }
    }

    recovery::close_session(session);
    Ok(())
}

fn prompt_restore_config(
    theme: &dyn Theme,
    session: &recovery::RecoverySession,
) -> Result<recovery::RestoreConfigInput, String> {
    let default_hostname = session
        .hostname
        .clone()
        .unwrap_or_else(|| "mitos".to_string());

    let hostname: String = Input::with_theme(theme)
        .with_prompt("Hostname")
        .default(default_hostname)
        .interact_text()
        .map_err(|e| e.to_string())?;

    let timezone: String = Input::with_theme(theme)
        .with_prompt("Timezone (e.g. America/New_York)")
        .default("UTC".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    let locale: String = Input::with_theme(theme)
        .with_prompt("Locale (e.g. en_US.UTF-8)")
        .default("en_US.UTF-8".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    let keyboard_layout: String = Input::with_theme(theme)
        .with_prompt("Keyboard layout (e.g. us)")
        .default("us".to_string())
        .interact_text()
        .map_err(|e| e.to_string())?;

    let reset_network = Confirm::with_theme(theme)
        .with_prompt("Reset network configuration to defaults?")
        .default(false)
        .interact()
        .map_err(|e| e.to_string())?;

    Ok(recovery::RestoreConfigInput {
        hostname,
        timezone,
        locale,
        keyboard_layout,
        reset_network,
    })
}
