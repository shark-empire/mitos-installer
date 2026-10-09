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

/// Configures the keyboard layout for both the Linux console (via /etc/vconsole.conf,
/// read by systemd-vconsole-setup on every boot) and X11/XWayland (via an xorg.conf.d
/// snippet, harmless to ship even if X11 is never used). `xkb_layout` is expected to be
/// an XKB layout code (e.g. "us", "de", "fr", "gb") since that's the naming convention
/// most people recognize; a small number of layouts use a different name for the Linux
/// console keymap, which `xkb_to_console_keymap` accounts for.
pub fn configure_keyboard(target_mount: &Path, xkb_layout: &str) -> Result<(), String> {
    let console_keymap = xkb_to_console_keymap(xkb_layout);

    let vconsole_path = target_mount.join("etc/vconsole.conf");
    fs::write(&vconsole_path, format!("KEYMAP={}\n", console_keymap))
        .map_err(|e| format!("Failed to write /etc/vconsole.conf: {}", e))?;

    let x11_dir = target_mount.join("etc/X11/xorg.conf.d");
    fs::create_dir_all(&x11_dir)
        .map_err(|e| format!("Failed to create {:?}: {}", x11_dir, e))?;

    let x11_conf = format!(
        "Section \"InputClass\"\n\
         \tIdentifier \"system-keyboard\"\n\
         \tMatchIsKeyboard \"on\"\n\
         \tOption \"XkbLayout\" \"{}\"\n\
         EndSection\n",
        xkb_layout
    );
    fs::write(x11_dir.join("00-keyboard.conf"), x11_conf)
        .map_err(|e| format!("Failed to write X11 keyboard config: {}", e))?;

    Ok(())
}

/// A handful of layouts are named differently for the Linux console keymap (loadkeys)
/// than for XKB. Everything not listed here uses the same name in both places.
fn xkb_to_console_keymap(xkb_layout: &str) -> &str {
    match xkb_layout {
        "gb" => "uk",
        other => other,
    }
}
