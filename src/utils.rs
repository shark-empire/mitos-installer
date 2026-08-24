use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Executes a shell command inside a chroot environment.
/// Optionally accepts a string slice to pipe into the command's standard input.
pub fn run_chroot_command(
    target_mount: &Path,
    command: &str,
    input: Option<&str>,
) -> Result<(), String> {
    let mut child = Command::new("chroot")
        .arg(target_mount.to_str().unwrap())
        .arg("sh")
        .arg("-c")
        .arg(command)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn chroot process: {}", e))?;

    // If input was provided, write it to the child's stdin
    if let Some(data) = input {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(data.as_bytes())
                .map_err(|e| format!("Failed to write to chroot stdin: {}", e))?;
        }
    }

    // Wait for the command to finish and capture output
    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed waiting for chroot command: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Chroot command '{}' failed: {}",
            command,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}
