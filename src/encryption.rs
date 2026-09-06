// In a new encryption.rs file
pub fn setup_luks(partition: &Path, password: &str) -> Result<PathBuf, String> {
    // Format as LUKS
    Command::new("cryptsetup")
        .args(["luksFormat", "--type", "luks2", partition.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().unwrap().write_all(password.as_bytes())
        }).map_err(|e| e.to_string())?;

    // Open the container
    let mapper_name = "mitos-root";
    Command::new("cryptsetup")
        .args(["open", partition.to_str().unwrap(), mapper_name])
        .status().map_err(|e| e.to_string())?;

    Ok(PathBuf::from(format!("/dev/mapper/{}", mapper_name)))
}
