use crate::disk;
use log::warn;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct CpuInfo {
    pub model_name: String,
    pub core_count: usize,
    pub architecture: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkInterfaceInfo {
    pub name: String,
    pub is_wireless: bool,
    pub is_up: bool,
}

/// A full snapshot of the detected hardware, gathered once and consulted throughout the
/// install (minimum-requirements checks, driver/service enablement, swap sizing, etc).
#[derive(Debug, Clone, Default)]
pub struct HardwareManifest {
    pub cpu: CpuInfo,
    pub ram_total_mib: u64,
    pub gpu_models: Vec<String>,
    pub has_nvidia: bool,
    pub has_amd_gpu: bool,
    pub has_intel_gpu: bool,
    pub is_laptop: bool,
    pub wifi_driver: Option<String>,
    pub network_interfaces: Vec<NetworkInterfaceInfo>,
    pub storage_device_count: usize,
}

/// Probes CPU, RAM, GPU, storage, and network hardware. Every probe degrades gracefully:
/// a missing tool (e.g. `lspci` not present in a minimal live environment) or unreadable
/// /proc or /sys file results in an empty/default value rather than a panic, since hardware
/// profiling should never be able to abort the installation on its own.
pub fn profile_hardware() -> HardwareManifest {
    let cpu = detect_cpu();
    let ram_total_mib = detect_ram_mib();
    let (gpu_models, has_nvidia, has_amd_gpu, has_intel_gpu) = detect_gpus();
    let is_laptop = Path::new("/sys/class/power_supply/BAT0").exists()
        || Path::new("/sys/class/power_supply/BAT1").exists();
    let network_interfaces = detect_network_interfaces();
    let wifi_driver = detect_wifi_driver(&network_interfaces);
    let storage_device_count = disk::get_available_disks().map(|d| d.len()).unwrap_or(0);

    HardwareManifest {
        cpu,
        ram_total_mib,
        gpu_models,
        has_nvidia,
        has_amd_gpu,
        has_intel_gpu,
        is_laptop,
        wifi_driver,
        network_interfaces,
        storage_device_count,
    }
}

fn detect_cpu() -> CpuInfo {
    let model_name = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                if key.trim().eq_ignore_ascii_case("model name") {
                    Some(value.trim().to_string())
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| "Unknown CPU".to_string());

    let core_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    CpuInfo {
        model_name,
        core_count,
        architecture: std::env::consts::ARCH,
    }
}

fn detect_ram_mib() -> u64 {
    let Ok(meminfo) = fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
            {
                return kb / 1024;
            }
        }
    }
    0
}

/// Returns (human-readable GPU model strings, has_nvidia, has_amd, has_intel).
fn detect_gpus() -> (Vec<String>, bool, bool, bool) {
    let output = Command::new("lspci").output();
    let Ok(output) = output else {
        warn!("lspci not available; skipping GPU detection.");
        return (Vec::new(), false, false, false);
    };
    if !output.status.success() {
        return (Vec::new(), false, false, false);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut models = Vec::new();
    let mut has_nvidia = false;
    let mut has_amd = false;
    let mut has_intel = false;

    for line in text.lines() {
        let lower = line.to_lowercase();
        let is_display_controller = lower.contains("vga compatible controller")
            || lower.contains("3d controller")
            || lower.contains("display controller");
        if !is_display_controller {
            continue;
        }

        // lspci lines look like "01:00.0 VGA compatible controller: NVIDIA Corporation ..."
        let description = line.split_once(": ").map(|(_, d)| d).unwrap_or(line);
        models.push(description.to_string());

        if lower.contains("nvidia") {
            has_nvidia = true;
        }
        if lower.contains("amd") || lower.contains("ati technologies") || lower.contains(" radeon")
        {
            has_amd = true;
        }
        if lower.contains("intel") {
            has_intel = true;
        }
    }

    (models, has_nvidia, has_amd, has_intel)
}

fn detect_network_interfaces() -> Vec<NetworkInterfaceInfo> {
    let mut interfaces = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return interfaces;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "lo" {
            continue;
        }

        let iface_path = entry.path();
        let is_wireless =
            iface_path.join("wireless").exists() || iface_path.join("phy80211").exists();
        let is_up = fs::read_to_string(iface_path.join("operstate"))
            .map(|s| s.trim() == "up")
            .unwrap_or(false);

        interfaces.push(NetworkInterfaceInfo {
            name,
            is_wireless,
            is_up,
        });
    }

    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces
}

fn detect_wifi_driver(interfaces: &[NetworkInterfaceInfo]) -> Option<String> {
    let wifi_iface = interfaces.iter().find(|i| i.is_wireless)?;
    let driver_link = Path::new("/sys/class/net")
        .join(&wifi_iface.name)
        .join("device/driver");

    fs::read_link(&driver_link)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// Checks if the host hardware meets the minimum requirements for MITOS. Takes an
/// already-gathered `HardwareManifest` (see `profile_hardware`) rather than probing
/// /proc/meminfo itself again, since callers need a full hardware profile anyway.
pub fn check_minimum_requirements(manifest: &HardwareManifest) -> Result<(), String> {
    const REQUIRED_RAM_MIB: u64 = 1024; // 1 GiB

    if manifest.ram_total_mib == 0 {
        warn!("Could not determine total RAM from /proc/meminfo; skipping RAM check.");
    } else if manifest.ram_total_mib < REQUIRED_RAM_MIB {
        return Err(format!(
            "Insufficient RAM. MITOS requires at least {} MiB (found {} MiB).",
            REQUIRED_RAM_MIB, manifest.ram_total_mib
        ));
    }

    let arch = std::env::consts::ARCH;
    if arch != "x86_64" && arch != "aarch64" {
        return Err(format!(
            "Unsupported CPU architecture '{}'. MITOS currently supports x86_64 and aarch64.",
            arch
        ));
    }

    Ok(())
}

/// Recommends a swap size in MiB from total RAM, following the common guided-partitioning
/// heuristic: more swap relative to RAM on small-memory machines (where swap is load-bearing),
/// tapering off on large-memory machines (where swap is mostly a safety net).
pub fn recommended_swap_mib(ram_total_mib: u64) -> u64 {
    match ram_total_mib {
        0..=2047 => ram_total_mib * 2,
        2048..=8191 => ram_total_mib,
        8192..=32767 => (ram_total_mib / 2).max(4096),
        _ => 4096,
    }
}
