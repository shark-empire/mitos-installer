use dialoguer::{Confirm, Select, theme::ColorfulTheme};
use std::process::Command;

fn main() {
    println!("Welcome to the MITOS Installer!\n");

    // 1. Disk Selection
    // In reality, you would programmatically read available disks from /sys/block
    let disks = vec!["/dev/sda (256GB NVMe)", "/dev/sdb (1TB HDD)", "Cancel"];
    
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select a disk to install MITOS onto")
        .default(0)
        .items(&disks)
        .interact()
        .unwrap();

    if selection == disks.len() - 1 {
        println!("Installation canceled by user.");
        return;
    }

    let target_disk = disks[selection].split_whitespace().next().unwrap();

    // 2. Warning Prompt
    let confirmation = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(&format!("WARNING: All data on {} will be destroyed. Continue?", target_disk))
        .default(false)
        .interact()
        .unwrap();

    if !confirmation {
        println!("Installation aborted.");
        return;
    }

    // 3. Execution (Simulated)
    println!("\nStarting installation...");
    
    println!("  -> Formatting {} as ext4...", target_disk);
    // Command::new("mkfs.ext4").arg(target_disk).status().unwrap();

    println!("  -> Mounting filesystem...");
    // Command::new("mount").args([target_disk, "/mnt"]).status().unwrap();

    println!("  -> Copying MITOS root filesystem...");
    // Command::new("cp").args(["-a", "/run/rootfs/*", "/mnt/"]).status().unwrap();

    println!("  -> Installing bootloader...");
    // Set up GRUB, systemd-boot, or Limine here

    println!("\nMITOS has been successfully installed!");
    println!("You may now reboot your system.");
}
