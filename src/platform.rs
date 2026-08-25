use log::info;
use std::fs;

/// Detects specific platform characteristics like virtualization
pub fn detect_platform() {
    if is_virtual_machine() {
        info!("Virtualization detected: Installer is running inside a VM.");
    } else {
        info!("Bare-metal detected: Installer is running on physical hardware.");
    }
}

fn is_virtual_machine() -> bool {
    // Check DMI sys_vendor for common hypervisor signatures
    if let Ok(vendor) = fs::read_to_string("/sys/class/dmi/id/sys_vendor") {
        let v = vendor.to_lowercase();
        if v.contains("qemu") || v.contains("virtualbox") || v.contains("vmware") {
            return true;
        }
    }

    // Fallback: check CPU info for hypervisor flag
    if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
        if cpuinfo.contains("hypervisor") {
            return true;
        }
    }

    false
}
