use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Returns true if `name` resolves to an executable on `$PATH` (a small, dependency-free
/// stand-in for the `which`/`command -v` shell builtins). Used to check that required
/// external tools are present before we start any destructive operation, and to make
/// best-effort integrations (e.g. optional services) skip themselves cleanly when the
/// corresponding binary isn't installed in the live/target environment.
pub fn command_exists(name: &str) -> bool {
    let Ok(path_var) = std::env::var("PATH") else {
        return false;
    };

    std::env::split_paths(&path_var).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

/// Hashes a plaintext password using SHA-512 via openssl. This is what `useradd` and
/// `chpasswd -e` expect. Shared by the interactive UI and the unattended answer-file
/// loader so there's exactly one place that decides how passwords get hashed.
pub fn hash_password(password: &str) -> Result<String, String> {
    let output = Command::new("openssl")
        .args(["passwd", "-6", "-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(password.as_bytes()).ok();
            }
            child.wait_with_output()
        })
        .map_err(|e| format!("Failed to hash password: {}", e))?;

    if !output.status.success() {
        return Err(
            "openssl passwd failed. Is openssl installed in the live environment?".to_string(),
        );
    }

    let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hash.is_empty() {
        return Err("Password hashing produced empty output.".to_string());
    }

    Ok(hash)
}

/// Best-effort overwrite of a secret's backing bytes with zeros before it's dropped.
///
/// Rust's `String`/`Vec<u8>` do not zero their heap allocation on drop, so a plaintext
/// passphrase can otherwise linger in freed memory for a while. This uses a volatile
/// write so the compiler cannot optimize the zeroing away, which is the same approach
/// dedicated crates like `zeroize` use internally. It is a defense-in-depth measure, not
/// a guarantee - copies made by the OS (swap, core dumps) are out of scope here.
pub fn secure_zero(secret: &mut String) {
    // Safety: we only write zero bytes, which are always valid UTF-8 (they form NUL
    // bytes, i.e. valid ASCII), so the string remains valid UTF-8 after this loop. The
    // buffer's length and capacity are unchanged, so no allocation invariants are broken.
    unsafe {
        let bytes = secret.as_bytes_mut();
        for byte in bytes.iter_mut() {
            std::ptr::write_volatile(byte, 0);
        }
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

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
