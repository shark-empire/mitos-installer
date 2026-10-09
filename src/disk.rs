use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Deserialize, Clone)]
struct LsblkOutput {
    blockdevices: Vec<BlockDevice>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BlockDevice {
    pub name: String,
    pub path: PathBuf,
    pub size: u64, // Size in bytes
    pub model: Option<String>,
    #[serde(rename = "type")]
    pub dev_type: String,
    pub ro: bool,
}

pub fn get_available_disks() -> Result<Vec<BlockDevice>, String> {
    // -J for JSON, -b for bytes, -o to specify exact columns
    let output = Command::new("lsblk")
        .args(["-J", "-b", "-o", "NAME,PATH,SIZE,MODEL,TYPE,RO"])
        .output()
        .map_err(|e| format!("Failed to execute lsblk: {}", e))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: LsblkOutput = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse lsblk JSON: {}", e))?;

    // Never offer the disk the live environment itself booted from as an install target -
    // wiping the media you're currently running off of is never something anyone actually
    // wants, and would corrupt the running installer mid-install.
    let live_disk = detect_live_boot_disk();

    // Filter for writable disks (ignore loop devices, roms, and live USBs)
    let valid_disks: Vec<BlockDevice> = parsed
        .blockdevices
        .into_iter()
        .filter(|dev| dev.dev_type == "disk" && !dev.ro)
        .filter(|dev| live_disk.as_deref() != Some(dev.name.as_str()))
        .collect();

    Ok(valid_disks)
}

/// Best-effort detection of which disk the live environment booted from. Only returns
/// `Some` when detection is unambiguous (the live root is backed by a real block device);
/// a live environment using an overlay/tmpfs root (common for squashfs-based ISOs) simply
/// results in `None` rather than a guess, so we never risk wrongly excluding a disk the
/// person actually wants to install to.
fn detect_live_boot_disk() -> Option<String> {
    let output = Command::new("findmnt").args(["-no", "SOURCE", "/"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let source = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !source.starts_with("/dev/") {
        return None;
    }

    let pkname_output = Command::new("lsblk")
        .args(["-no", "PKNAME", &source])
        .output()
        .ok()?;
    if !pkname_output.status.success() {
        return None;
    }
    let pkname = String::from_utf8_lossy(&pkname_output.stdout).trim().to_string();

    if pkname.is_empty() {
        // `source` may already be a whole-disk device (no separate partition), e.g. a live
        // environment whose root sits directly on a partitionless block device.
        let bare_name = source.trim_start_matches("/dev/").to_string();
        if bare_name.is_empty() {
            None
        } else {
            Some(bare_name)
        }
    } else {
        Some(pkname)
    }
}
