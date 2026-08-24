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

fn main() {
    logging::init_logger().expect("Failed to initialize system log framework");

    println!("Starting MITOS Operating System Installer...");

    let mut pipeline = InstallerPipeline::new();

    if let Err(err) = pipeline.execute() {
        eprintln!("\nInstallation failed: {}", err);
        recovery::trigger_emergency_cleanup();
        std::process::exit(1);
    }

    println!("\nInstallation finished successfully. Reboot to launch MITOS.");
}
