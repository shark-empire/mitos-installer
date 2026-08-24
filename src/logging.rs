use simplelog::*;
use std::fs::File;

pub fn init_logger() -> Result<(), String> {
    let log_file_path = "/var/log/mitos-install.log";
    
    let file = File::create(log_file_path)
        .map_err(|e| format!("Failed to create log file at {}: {}", log_file_path, e))?;

    CombinedLogger::init(vec![
        // Console output for the user (only Info and above)
        TermLogger::new(
            LevelFilter::Info,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        // Detailed file output for debugging (Debug and above)
        WriteLogger::new(
            LevelFilter::Debug,
            Config::default(),
            file,
        ),
    ]).map_err(|e| format!("Failed to initialize logger: {}", e))?;

    log::info!("MITOS Installer logging initialized.");
    Ok(())
}
