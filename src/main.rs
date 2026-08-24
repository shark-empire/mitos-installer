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
use std::path::Path;

fn main() {
    logging::init_logger().expect("Failed to initialize system log framework");

    let mut pipeline = installer::InstallerPipeline::new();

    if let Err(e) = ui::run_interactive_setup(&mut pipeline.ctx) {
        eprintln!("\nSetup cancelled: {}", e);
        std::process::exit(1);
    }

    println!("\nCommencing installation...");
    if let Err(err) = pipeline.execute() {
        eprintln!("\nInstallation failed: {}", err);
        
        // Extract the target disk path if it was successfully selected during setup
        let disk_path = pipeline.ctx.target.as_ref().map(|t| t.device_path.as_path());
        let mount_point = Path::new(mount::DEFAULT_TARGET_MOUNT);
        
        recovery::trigger_emergency_cleanup(mount_point, disk_path);
        
        std::process::exit(1);
    }

    println!("\nMITOS installed successfully! You may now reboot.");
}

