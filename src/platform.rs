use log::info;
use std::fs;
use std::path::Path;

/// Which firmware interface the running (live) environment was booted with. This is what
/// determines whether we install Limine's EFI or BIOS bootloader stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    Uefi,
    Bios,
}

impl BootMode {
    pub fn label(self) -> &'static str {
        match self {
            BootMode::Uefi => "UEFI",
            BootMode::Bios => "Legacy BIOS",
        }
    }
}

/// Snapshot of platform characteristics gathered once at startup.
#[derive(Debug, Clone, Copy)]
pub struct Platform {
    pub boot_mode: BootMode,
    pub is_virtual_machine: bool,
}

/// Detects the current firmware boot mode and whether we're running inside a VM, logging
/// both for the transaction/debug log, and returns the result for the caller to act on.
pub fn detect_platform() -> Platform {
    let boot_mode = detect_boot_mode();
    let is_virtual_machine = is_virtual_machine();

    match boot_mode {
        BootMode::Uefi => info!("Firmware: UEFI boot detected (/sys/firmware/efi present)."),
        BootMode::Bios => info!("Firmware: Legacy BIOS boot detected (no /sys/firmware/efi)."),
    }

    if is_virtual_machine {
        info!("Virtualization detected: installer is running inside a VM.");
    } else {
        info!("Bare-metal detected: installer is running on physical hardware.");
    }

    Platform {
        boot_mode,
        is_virtual_machine,
    }
}

/// The presence of /sys/firmware/efi is the standard, reliable way to tell whether the
/// currently running kernel was booted via UEFI. Its absence means legacy BIOS/CSM boot.
pub fn detect_boot_mode() -> BootMode {
    if Path::new("/sys/firmware/efi").exists() {
        BootMode::Uefi
    } else {
        BootMode::Bios
    }
}

fn is_virtual_machine() -> bool {
    // Check DMI sys_vendor for common hypervisor signatures
    if let Ok(vendor) = fs::read_to_string("/sys/class/dmi/id/sys_vendor") {
        let v = vendor.to_lowercase();
        if v.contains("qemu")
            || v.contains("virtualbox")
            || v.contains("vmware")
            || v.contains("kvm")
            || v.contains("microsoft corporation") // Hyper-V reports this as sys_vendor
            || v.contains("xen")
            || v.contains("bochs")
            || v.contains("parallels")
        {
            return true;
        }
    }

    // Fallback: check CPU info for the hypervisor flag
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        if cpuinfo.contains("hypervisor") {
            return true;
        }
    }

    false
}
