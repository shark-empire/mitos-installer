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

    // Filter for writable disks (ignore loop devices, roms, and live USBs)
    let valid_disks: Vec<BlockDevice> = parsed
        .blockdevices
        .into_iter()
        .filter(|dev| dev.dev_type == "disk" && !dev.ro)
        .collect();

    Ok(valid_disks)
}
