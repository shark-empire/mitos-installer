mod answerfile;
mod bootloader;
mod config;
mod disk;
mod encryption;
mod filesystem;
mod hardware;
mod init;
mod installer;
mod kernel;
mod locale;
mod logging;
mod mount;
mod network;
mod partition;
mod platform;
mod profile;
mod recovery;
mod rootfs;
mod security;
mod ui;
mod users;
mod utils;
mod verify;

rust_i18n::i18n!("locales", fallback = "en-US");

use installer::{InstallerPipeline, ProgressReporter};
use logging::TransactionLog;
use std::path::PathBuf;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Interactive,
    Unattended,
    Recovery,
}

/// `graphical` is deliberately orthogonal to `mode`: it only picks the dialoguer theme
/// (see `ui.rs`), and applies to both the interactive wizard and the recovery menu, so
/// `--recovery --text` is a valid combination alongside plain `--text`.
struct ParsedArgs {
    mode: Mode,
    graphical: bool,
    answer_path: Option<PathBuf>,
}

fn main() {
    let args = parse_args();

    if let Err(e) = logging::init_logger() {
        eprintln!("Failed to initialize logging: {}", e);
        std::process::exit(1);
    }

    let result = match args.mode {
        Mode::Interactive => run_interactive(args.graphical),
        Mode::Unattended => run_unattended(args.answer_path),
        Mode::Recovery => run_recovery(args.graphical),
    };

    if let Err(e) = result {
        log::error!("{}", e);
        eprintln!("\nError: {}", e);
        std::process::exit(1);
    }
}

fn parse_args() -> ParsedArgs {
    let args: Vec<String> = std::env::args().collect();
    let mut mode = Mode::Interactive;
    let mut graphical = true;
    let mut answer_path: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--graphical" => graphical = true,
            "--text" => graphical = false,
            "--recovery" => mode = Mode::Recovery,
            "--unattended" => {
                mode = Mode::Unattended;
                if let Some(path) = args.get(i + 1) {
                    if !path.starts_with("--") {
                        answer_path = Some(PathBuf::from(path));
                        i += 1;
                    }
                }
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("Unknown argument: '{}'. Use --help for usage.", other);
                std::process::exit(2);
            }
        }
        i += 1;
    }

    ParsedArgs {
        mode,
        graphical,
        answer_path,
    }
}

fn print_help() {
    println!("MITOS Installer\n");
    println!("Usage: mitos-installer [MODE] [--graphical | --text]\n");
    println!("Modes (default: interactive install):");
    println!("  --unattended <file>   Unattended install driven by a JSON answer file");
    println!("  --recovery            Recovery mode for an existing MITOS installation");
    println!();
    println!("Theme (combinable with the default mode or --recovery; ignored by --unattended):");
    println!("  --graphical           Full-color TUI (default)");
    println!("  --text                Plain, minimal TUI - for serial consoles, etc.");
    println!();
    println!("  -h, --help            Show this help message");
}

/// Runs the interactive wizard (graphical or text theme), then executes the pipeline. On
/// failure, rolls the target disk back to a clean state via `recovery::trigger_emergency_cleanup`.
fn run_interactive(graphical: bool) -> Result<(), String> {
    let mut pipeline = InstallerPipeline::new();
    ui::run_interactive_setup(&mut pipeline.ctx, graphical)?;

    let txn_log = TransactionLog::open_default();

    if graphical {
        let mut progress = IndicatifProgress::new();
        run_pipeline_with_rollback(&mut pipeline, &txn_log, &mut progress)
    } else {
        let mut progress = PlainProgress;
        run_pipeline_with_rollback(&mut pipeline, &txn_log, &mut progress)
    }
}

/// Loads and validates a JSON answer file, resolves it into an `InstallationContext`
/// exactly like the interactive wizard would build one, then executes the pipeline with
/// no prompts. See `answerfile.rs` for the file format and required fields.
fn run_unattended(answer_path: Option<PathBuf>) -> Result<(), String> {
    let path = answer_path.ok_or_else(|| {
        "`--unattended` requires a path to a JSON answer file, e.g. `--unattended \
         /path/to/answers.json`."
            .to_string()
    })?;

    let answers = answerfile::AnswerFile::load(&path)?;
    let ram_total_mib = hardware::profile_hardware().ram_total_mib;

    let mut pipeline = InstallerPipeline::new();
    pipeline.ctx = answers.resolve(ram_total_mib)?;
    rust_i18n::set_locale(&pipeline.ctx.ui_language);

    // Cross-check the requested disk against what we consider safe/valid targets
    // (writable, not the live boot medium) before touching anything.
    let requested_disk = pipeline
        .ctx
        .target
        .as_ref()
        .map(|t| t.device_path.clone())
        .ok_or("Internal error: answer file did not resolve to a target disk.")?;
    let available = disk::get_available_disks()?;
    if !available.iter().any(|d| d.path == requested_disk) {
        return Err(format!(
            "Disk {:?} from the answer file is not in the list of available, writable \
             disks on this system.",
            requested_disk
        ));
    }

    let txn_log = TransactionLog::open_default();
    let mut progress = PlainProgress;
    run_pipeline_with_rollback(&mut pipeline, &txn_log, &mut progress)
}

fn run_pipeline_with_rollback(
    pipeline: &mut InstallerPipeline,
    txn_log: &TransactionLog,
    progress: &mut dyn ProgressReporter,
) -> Result<(), String> {
    match pipeline.execute(txn_log, progress) {
        Ok(()) => {
            println!("\nInstallation complete! You can now reboot into MITOS.");
            Ok(())
        }
        Err(e) => {
            progress.fail(&e);
            let (target_mount, target_disk, swap_partition, luks_mapper) = pipeline.cleanup_info();
            recovery::trigger_emergency_cleanup(
                &target_mount,
                target_disk.as_deref(),
                swap_partition.as_deref(),
                luks_mapper,
            );
            Err(e)
        }
    }
}

fn run_recovery(graphical: bool) -> Result<(), String> {
    let txn_log = TransactionLog::new("/var/log/mitos-recovery-transactions.log");
    ui::run_recovery_menu(graphical, &txn_log)
}

// -----------------------------------------------------------------------------------
// Progress reporter implementations
// -----------------------------------------------------------------------------------

/// Graphical mode: an animated spinner with a status message, via indicatif.
struct IndicatifProgress {
    bar: ProgressBar,
}

impl IndicatifProgress {
    fn new() -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_style(ProgressStyle::default_spinner());
        bar.enable_steady_tick(Duration::from_millis(120));
        Self { bar }
    }
}

impl ProgressReporter for IndicatifProgress {
    fn step(&mut self, message: &str) {
        self.bar.set_message(message.to_string());
    }
    fn finish(&mut self, message: &str) {
        self.bar.finish_with_message(message.to_string());
    }
    fn fail(&mut self, message: &str) {
        self.bar
            .abandon_with_message(format!("Failed: {}", message));
    }
}

/// Text/unattended mode: plain status lines, no ANSI/animation - safe for serial
/// consoles, logs, and any terminal that doesn't handle carriage-return redraws well.
struct PlainProgress;

impl ProgressReporter for PlainProgress {
    fn step(&mut self, message: &str) {
        println!("-> {}", message);
    }
    fn finish(&mut self, message: &str) {
        println!("{}", message);
    }
    fn fail(&mut self, message: &str) {
        println!("Failed: {}", message);
    }
}
