use crate::utils::run_chroot_command;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::Path;

/// Configures system locale and timezone in the target environment
pub fn configure_locale(target_mount: &Path, locale: &str, timezone: &str) -> Result<(), String> {
    // 1. Configure Timezone
    let localtime_path = target_mount.join("etc/localtime");
    if localtime_path.exists() || localtime_path.is_symlink() {
        fs::remove_file(&localtime_path)
            .map_err(|e| format!("Failed to remove existing /etc/localtime: {}", e))?;
    }

    // e.g., symlink /etc/localtime -> ../usr/share/zoneinfo/Africa/Accra
    let zoneinfo_path = format!("../usr/share/zoneinfo/{}", timezone);
    symlink(&zoneinfo_path, &localtime_path)
        .map_err(|e| format!("Failed to symlink timezone {}: {}", timezone, e))?;

    // 2. Set default language environment variable
    let locale_conf_path = target_mount.join("etc/locale.conf");
    fs::write(&locale_conf_path, format!("LANG={}\n", locale))
        .map_err(|e| format!("Failed to write /etc/locale.conf: {}", e))?;

    // 3. Append the chosen locale to locale.gen to ensure it gets compiled
    let locale_gen_path = target_mount.join("etc/locale.gen");
    let mut locale_gen_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&locale_gen_path)
        .map_err(|e| format!("Failed to open /etc/locale.gen: {}", e))?;

    writeln!(locale_gen_file, "{} UTF-8", locale)
        .map_err(|e| format!("Failed to append to /etc/locale.gen: {}", e))?;

    // 4. Generate the locale binaries via chroot
    run_chroot_command(target_mount, "locale-gen", None)?;

    Ok(())
}
