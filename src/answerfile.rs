//! Unattended installer mode: reads a JSON answer file describing every choice the
//! interactive wizard would otherwise ask for, validates it, and resolves it into the
//! same `InstallationContext` the interactive UI builds - so both modes drive the exact
//! same `InstallerPipeline::execute()`.

use crate::installer::{
    EncryptionConfig, InstallationContext, SwapChoice, SystemConfig, TargetDisk,
};
use crate::{config, mount, profile, utils};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur while loading and validating an answer file. Kept as a proper
/// `thiserror` enum - rather than the `Result<_, String>` convention used through most of
/// the rest of the crate - because this module is self-contained (nothing else needs to
/// pattern-match on these variants) and parsing/validation naturally has several distinct
/// failure shapes worth telling apart in a bug report.
#[derive(Debug, Error)]
pub enum AnswerFileError {
    #[error("failed to read answer file '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse '{path}' as JSON: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("answer file field '{field}' has an invalid value: {reason}")]
    InvalidValue { field: String, reason: String },

    #[error(
        "unattended installs are destructive and require \"confirm_destructive\": true at \
         the top level of the answer file, as an explicit acknowledgement that the target \
         disk will be erased"
    )]
    NotConfirmed,
}

/// Converts to the crate-wide `Result<_, String>` convention at the boundary where
/// unattended mode hands off to the rest of the installer (see `main.rs`).
impl From<AnswerFileError> for String {
    fn from(e: AnswerFileError) -> String {
        e.to_string()
    }
}

#[derive(Debug, Deserialize)]
pub struct AnswerFile {
    /// Must be explicitly set to `true`. A missing or `false` value refuses to proceed -
    /// see `AnswerFileError::NotConfirmed`.
    #[serde(default)]
    pub confirm_destructive: bool,

    /// Installer UI language, e.g. "en-US". Defaults to "en-US" if omitted.
    #[serde(default = "default_language")]
    pub language: String,

    /// Target disk device path, e.g. "/dev/sda". Required.
    pub disk: String,

    /// "ext4" or "btrfs". Defaults to "ext4".
    #[serde(default = "default_filesystem")]
    pub filesystem: String,

    /// If true, size swap automatically from detected RAM (overrides `swap_mib`).
    #[serde(default)]
    pub swap_auto: bool,
    /// Explicit swap size in MiB. 0 or omitted (with `swap_auto` false) means no swap.
    #[serde(default)]
    pub swap_mib: u64,

    #[serde(default)]
    pub encrypt: bool,
    pub encryption_passphrase: Option<String>,

    pub hostname: String,
    pub username: String,
    /// Plaintext password. Hashed via `utils::hash_password` as soon as the answer file
    /// is resolved, and never written back to disk in plaintext.
    pub password: String,
    pub timezone: String,
    pub locale: String,
    #[serde(default = "default_keyboard_layout")]
    pub keyboard_layout: String,

    /// "minimal" | "standard" | "gaming" | "creator". Defaults to "standard".
    #[serde(default = "default_install_profile")]
    pub install_profile: String,
    /// "graphical" | "server". Defaults to "graphical".
    #[serde(default = "default_desktop")]
    pub desktop: String,

    /// Optional Wi-Fi credentials to carry over into the installed system.
    pub wifi_ssid: Option<String>,
    pub wifi_password: Option<String>,
}

fn default_language() -> String {
    "en-US".to_string()
}
fn default_filesystem() -> String {
    "ext4".to_string()
}
fn default_keyboard_layout() -> String {
    "us".to_string()
}
fn default_install_profile() -> String {
    "standard".to_string()
}
fn default_desktop() -> String {
    "graphical".to_string()
}

impl AnswerFile {
    pub fn load(path: &Path) -> Result<Self, AnswerFileError> {
        let contents = fs::read_to_string(path).map_err(|e| AnswerFileError::Io {
            path: path.display().to_string(),
            source: e,
        })?;

        let parsed: AnswerFile =
            serde_json::from_str(&contents).map_err(|e| AnswerFileError::Parse {
                path: path.display().to_string(),
                source: e,
            })?;

        parsed.validate()?;
        Ok(parsed)
    }

    fn validate(&self) -> Result<(), AnswerFileError> {
        if !self.confirm_destructive {
            return Err(AnswerFileError::NotConfirmed);
        }
        if self.disk.trim().is_empty() {
            return Err(AnswerFileError::InvalidValue {
                field: "disk".to_string(),
                reason: "must not be empty".to_string(),
            });
        }
        if self.hostname.trim().is_empty() {
            return Err(AnswerFileError::InvalidValue {
                field: "hostname".to_string(),
                reason: "must not be empty".to_string(),
            });
        }
        if self.username.trim().is_empty() {
            return Err(AnswerFileError::InvalidValue {
                field: "username".to_string(),
                reason: "must not be empty".to_string(),
            });
        }
        if self.password.is_empty() {
            return Err(AnswerFileError::InvalidValue {
                field: "password".to_string(),
                reason: "must not be empty".to_string(),
            });
        }
        if self.timezone.trim().is_empty() {
            return Err(AnswerFileError::InvalidValue {
                field: "timezone".to_string(),
                reason: "must not be empty, e.g. \"America/New_York\"".to_string(),
            });
        }
        if self.locale.trim().is_empty() {
            return Err(AnswerFileError::InvalidValue {
                field: "locale".to_string(),
                reason: "must not be empty, e.g. \"en_US.UTF-8\"".to_string(),
            });
        }
        parse_filesystem(&self.filesystem)?;
        parse_install_profile(&self.install_profile)?;
        parse_desktop_choice(&self.desktop)?;

        if self.encrypt
            && self
                .encryption_passphrase
                .as_deref()
                .map(str::is_empty)
                .unwrap_or(true)
        {
            return Err(AnswerFileError::InvalidValue {
                field: "encryption_passphrase".to_string(),
                reason: "must be set (and non-empty) when \"encrypt\" is true".to_string(),
            });
        }

        Ok(())
    }

    /// Resolves this answer file into the same `InstallationContext` shape the
    /// interactive UI builds. `ram_total_mib` comes from `hardware::profile_hardware()`,
    /// needed here only to size swap when `swap_auto` is set.
    pub fn resolve(&self, ram_total_mib: u64) -> Result<InstallationContext, String> {
        let password_hash = utils::hash_password(&self.password)?;

        let swap = if self.swap_auto {
            SwapChoice::Auto
        } else if self.swap_mib > 0 {
            SwapChoice::Sized(self.swap_mib)
        } else {
            SwapChoice::None
        };
        // Auto is resolved to a concrete size immediately so downstream code (partition
        // sizing, capacity validation) only ever has to deal with None/Sized.
        let swap = match swap {
            SwapChoice::Auto => {
                SwapChoice::Sized(crate::hardware::recommended_swap_mib(ram_total_mib))
            }
            other => other,
        };

        let encryption = EncryptionConfig {
            enabled: self.encrypt,
            passphrase: self.encryption_passphrase.clone().unwrap_or_default(),
        };

        let wifi_credentials = match (&self.wifi_ssid, &self.wifi_password) {
            (Some(ssid), Some(pass)) if !ssid.is_empty() => Some((ssid.clone(), pass.clone())),
            _ => None,
        };

        Ok(InstallationContext {
            target: Some(TargetDisk::new(
                PathBuf::from(&self.disk),
                PathBuf::from(mount::DEFAULT_TARGET_MOUNT),
            )),
            sys_config: SystemConfig {
                hostname: self.hostname.clone(),
                username: self.username.clone(),
                password_hash,
                timezone: self.timezone.clone(),
                locale: self.locale.clone(),
                keyboard_layout: self.keyboard_layout.clone(),
            },
            is_uefi: false, // overwritten by execute() from real platform detection
            fs_type: parse_filesystem(&self.filesystem)?,
            swap,
            encryption,
            install_profile: parse_install_profile(&self.install_profile)?,
            desktop_choice: parse_desktop_choice(&self.desktop)?,
            ui_language: self.language.clone(),
            wifi_credentials,
        })
    }
}

fn parse_filesystem(value: &str) -> Result<config::FilesystemType, AnswerFileError> {
    match value.to_lowercase().as_str() {
        "ext4" => Ok(config::FilesystemType::Ext4),
        "btrfs" => Ok(config::FilesystemType::Btrfs),
        other => Err(AnswerFileError::InvalidValue {
            field: "filesystem".to_string(),
            reason: format!("'{}' is not one of: ext4, btrfs", other),
        }),
    }
}

fn parse_install_profile(value: &str) -> Result<profile::InstallProfile, AnswerFileError> {
    match value.to_lowercase().as_str() {
        "minimal" => Ok(profile::InstallProfile::Minimal),
        "standard" => Ok(profile::InstallProfile::Standard),
        "gaming" => Ok(profile::InstallProfile::Gaming),
        "creator" => Ok(profile::InstallProfile::Creator),
        other => Err(AnswerFileError::InvalidValue {
            field: "install_profile".to_string(),
            reason: format!(
                "'{}' is not one of: minimal, standard, gaming, creator",
                other
            ),
        }),
    }
}

fn parse_desktop_choice(value: &str) -> Result<profile::DesktopChoice, AnswerFileError> {
    match value.to_lowercase().as_str() {
        "graphical" | "desktop" => Ok(profile::DesktopChoice::Graphical),
        "server" | "headless" => Ok(profile::DesktopChoice::HeadlessServer),
        other => Err(AnswerFileError::InvalidValue {
            field: "desktop".to_string(),
            reason: format!("'{}' is not one of: graphical, server", other),
        }),
    }
}
