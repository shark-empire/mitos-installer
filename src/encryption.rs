use log::info;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The device-mapper name used for the encrypted root volume, both at install time and
/// (via /etc/crypttab + the `rd.luks.name=` kernel parameter) on every subsequent boot.
pub const ROOT_MAPPER_NAME: &str = "mitos-root";

/// Formats `partition` as LUKS2 and opens it, returning the path to the resulting
/// `/dev/mapper/...` device that should be used everywhere in place of the raw partition
/// (formatting, mounting, etc). The raw partition path itself is still needed afterwards
/// for /etc/crypttab and the kernel command line, so callers should hang on to it
/// separately rather than discarding it.
pub fn setup_luks(partition: &Path, passphrase: &str) -> Result<PathBuf, String> {
    luks_format(partition, passphrase)?;
    open_luks(partition, ROOT_MAPPER_NAME, passphrase)?;
    Ok(mapper_path(ROOT_MAPPER_NAME))
}

pub fn mapper_path(mapper_name: &str) -> PathBuf {
    PathBuf::from(format!("/dev/mapper/{}", mapper_name))
}

fn luks_format(partition: &Path, passphrase: &str) -> Result<(), String> {
    let part_str = partition.to_str().ok_or("Invalid partition path")?;
    info!("Formatting {:?} as LUKS2...", partition);

    // Passing "-" as the (positional) key file makes cryptsetup read the passphrase from
    // stdin *and* skip the interactive "Type 'yes' to continue" confirmation, which is
    // exactly what a non-interactive installer needs. --batch-mode is added for the same
    // reason, belt-and-suspenders, in case of version differences in cryptsetup's prompt
    // behavior. We deliberately do not write a trailing newline: cryptsetup reads the
    // passphrase from a "-" key file up to EOF, not just up to the first newline, so
    // closing stdin (rather than terminating with '\n') is what marks the end of input.
    let mut child = Command::new("cryptsetup")
        .args([
            "--batch-mode",
            "luksFormat",
            "--type",
            "luks2",
            part_str,
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn cryptsetup luksFormat: {}", e))?;

    write_passphrase_and_close_stdin(&mut child, passphrase)?;

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed waiting for cryptsetup luksFormat: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "cryptsetup luksFormat failed on {:?}: {}",
            partition,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}

/// Opens an *existing* LUKS container (does not format/initialize it). Used both right
/// after `luks_format` during a fresh install, and on its own by recovery mode to unlock
/// a previously-encrypted installation for repair.
pub fn open_luks(partition: &Path, mapper_name: &str, passphrase: &str) -> Result<(), String> {
    let part_str = partition.to_str().ok_or("Invalid partition path")?;
    info!(
        "Opening LUKS container {:?} as /dev/mapper/{}...",
        partition, mapper_name
    );

    let mut child = Command::new("cryptsetup")
        .args(["open", "--key-file", "-", part_str, mapper_name])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn cryptsetup open: {}", e))?;

    write_passphrase_and_close_stdin(&mut child, passphrase)?;

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed waiting for cryptsetup open: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "cryptsetup open failed for {:?}: {}",
            partition,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}

fn write_passphrase_and_close_stdin(child: &mut Child, passphrase: &str) -> Result<(), String> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or("Failed to open cryptsetup's stdin pipe")?;

    stdin
        .write_all(passphrase.as_bytes())
        .map_err(|e| format!("Failed to write passphrase to cryptsetup: {}", e))?;

    // Explicitly close stdin so cryptsetup sees EOF right after the passphrase bytes,
    // instead of blocking waiting for more input.
    drop(stdin);
    Ok(())
}

/// Closes (deactivates) an open LUKS mapping. Used both at the very end of a successful
/// install (after everything is unmounted) and during emergency rollback, since a failed
/// disk can't be safely re-partitioned while a dm-crypt mapping on it is still open.
pub fn close_luks(mapper_name: &str) -> Result<(), String> {
    let status = Command::new("cryptsetup")
        .args(["close", mapper_name])
        .status()
        .map_err(|e| format!("Failed to execute cryptsetup close: {}", e))?;

    if !status.success() {
        return Err(format!(
            "cryptsetup close failed for mapper '{}'",
            mapper_name
        ));
    }
    Ok(())
}

/// Returns the LUKS container's own UUID (distinct from the filesystem UUID of whatever
/// lives inside it). This is what /etc/crypttab and the `rd.luks.uuid=` kernel parameter
/// need, so the initramfs can find and unlock the right partition at boot.
pub fn luks_uuid(partition: &Path) -> Result<String, String> {
    let part_str = partition.to_str().ok_or("Invalid partition path")?;
    let output = Command::new("cryptsetup")
        .args(["luksUUID", part_str])
        .output()
        .map_err(|e| format!("Failed to execute cryptsetup luksUUID: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "cryptsetup luksUUID failed for {:?}: {}",
            partition,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uuid.is_empty() {
        return Err(format!(
            "cryptsetup luksUUID returned empty for {:?}",
            partition
        ));
    }
    Ok(uuid)
}
