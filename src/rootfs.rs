use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub enum RootfsSource {
    Archive(String),   // Path to tarball, e.g. "/run/media/rootfs.tar.xz"
    Directory(String), // Path to live rootfs, e.g. "/run/rootfs"
}

/// Unpacks or syncs the rootfs payload onto the mounted root directory
pub fn deploy_rootfs(source: &RootfsSource, target_dir: &Path) -> Result<(), String> {
    match source {
        RootfsSource::Archive(tar_path) => extract_tarball(tar_path, target_dir),
        RootfsSource::Directory(source_dir) => sync_directory(source_dir, target_dir),
    }
}

fn extract_tarball(tar_path: &str, target_dir: &Path) -> Result<(), String> {
    let target_str = target_dir.to_str().ok_or("Invalid target directory path")?;

    let output = Command::new("tar")
        .args(["-xpf", tar_path, "-C", target_str, "--numeric-owner"])
        .output()
        .map_err(|e| format!("Failed to execute tar extraction: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to extract rootfs archive from {}: {}",
            tar_path,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

fn sync_directory(source_dir: &str, target_dir: &Path) -> Result<(), String> {
    let target_str = target_dir.to_str().ok_or("Invalid target directory path")?;
    let src_trailing = format!("{}/", source_dir.trim_end_matches('/'));

    let output = Command::new("rsync")
        .args([
            "-aHAX",
            "--numeric-ids",
            "--info=progress2",
            &src_trailing,
            target_str,
        ])
        .output()
        .map_err(|e| format!("Failed to execute rsync: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to copy live rootfs from {}: {}",
            source_dir,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}
