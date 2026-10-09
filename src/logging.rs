use simplelog::*;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Free-form debug/info log for humans reading along or troubleshooting after the fact.
pub const DEFAULT_LOG_PATH: &str = "/var/log/mitos-install.log";
/// Structured, append-only record of each major pipeline step, used by recovery mode's
/// "inspect logs" action. Kept separate from the free-form log above so it stays easy to
/// parse and review at a glance rather than needing to grep through debug output.
pub const DEFAULT_TRANSACTION_LOG_PATH: &str = "/var/log/mitos-install-transactions.log";

pub fn init_logger() -> Result<(), String> {
    init_logger_at(DEFAULT_LOG_PATH)
}

/// Same as `init_logger`, but lets the caller choose the log path (recovery mode uses a
/// different file than a fresh install). Falls back to a file in the system temp
/// directory if the preferred path isn't writable, rather than making the whole program
/// unusable just because logging couldn't be set up exactly as requested.
pub fn init_logger_at(preferred_path: &str) -> Result<(), String> {
    let file = match File::create(preferred_path) {
        Ok(f) => f,
        Err(primary_err) => {
            let fallback = std::env::temp_dir().join("mitos-install.log");
            eprintln!(
                "Warning: could not create log file at {} ({}); falling back to {:?}",
                preferred_path, primary_err, fallback
            );
            File::create(&fallback).map_err(|fallback_err| {
                format!(
                    "Failed to create log file at {} ({}), and fallback {:?} also failed: {}",
                    preferred_path, primary_err, fallback, fallback_err
                )
            })?
        }
    };

    CombinedLogger::init(vec![
        // Console output for the user (only Info and above)
        TermLogger::new(
            LevelFilter::Info,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        // Detailed file output for debugging (Debug and above)
        WriteLogger::new(LevelFilter::Debug, Config::default(), file),
    ])
    .map_err(|e| format!("Failed to initialize logger: {}", e))?;

    log::info!("MITOS Installer logging initialized.");
    Ok(())
}

/// A simple, append-only, timestamped record of each major installation step. This is
/// the "installation transaction log" a real installer needs for two reasons: it gives
/// the person a plain record of exactly what was done to their disk, and it's what
/// recovery mode's "inspect logs" action reads back.
#[derive(Debug, Clone)]
pub struct TransactionLog {
    path: PathBuf,
}

impl TransactionLog {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn open_default() -> Self {
        Self::new(DEFAULT_TRANSACTION_LOG_PATH)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one entry. This is intentionally infallible from the caller's point of
    /// view (a logging hiccup should never abort an in-progress installation) - any
    /// failure to write is itself reported through the regular `log` crate instead.
    pub fn record(&self, step: &str) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!("{} {}\n", timestamp, step);

        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| f.write_all(line.as_bytes()));

        if let Err(e) = result {
            log::warn!(
                "Failed to write transaction log entry to {:?}: {}",
                self.path,
                e
            );
        }
    }

    /// Reads back every recorded entry as (unix timestamp, description) pairs, oldest
    /// first, for display in recovery mode.
    pub fn read_entries(&self) -> Result<Vec<(u64, String)>, String> {
        let contents = fs::read_to_string(&self.path)
            .map_err(|e| format!("Failed to read transaction log {:?}: {}", self.path, e))?;

        Ok(contents
            .lines()
            .filter_map(|line| {
                let (ts, desc) = line.split_once(' ')?;
                let ts: u64 = ts.parse().ok()?;
                Some((ts, desc.to_string()))
            })
            .collect())
    }
}
