mod bootloader;
mod config;
mod disk;
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
mod recovery;
mod rootfs;
mod security;
mod ui;
mod users;
mod verify;

use installer::InstallerPipeline;

// In src/main.rs (inside the main function)

fn main() {
    // 1. Initialize logging
    logging::init_logger().expect("Failed to initialize system log framework");

    // 2. Create the pipeline context
    let mut pipeline = installer::InstallerPipeline::new();

    // 3. Run interactive UI to gather user choices
    if let Err(e) = ui::run_interactive_setup(&mut pipeline.ctx) {
        eprintln!("\nSetup cancelled: {}", e);
        std::process::exit(1);
    }

    // 4. Execute the installation pipeline
    println!("\nCommencing installation...");
    if let Err(err) = pipeline.execute() {
        eprintln!("\nInstallation failed: {}", err);
        // recovery::trigger_emergency_cleanup(); // (To be implemented)
        std::process::exit(1);
    }

    println!("\nMITOS installed successfully! You may now reboot.");
}

