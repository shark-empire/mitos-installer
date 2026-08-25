use std::fs;

/// Checks if the host hardware meets the minimum requirements for MITOS
pub fn check_minimum_requirements() -> Result<(), String> {
    let required_ram_kb = 1_048_576; // 1 GB in KB

    let meminfo = fs::read_to_string("/proc/meminfo")
        .map_err(|e| format!("Failed to read /proc/meminfo: {}", e))?;

    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(ram_kb) = parts[1].parse::<u64>() {
                    if ram_kb < required_ram_kb {
                        return Err(format!(
                            "Insufficient RAM. MITOS requires at least 1GB (Found {}MB).",
                            ram_kb / 1024
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}
