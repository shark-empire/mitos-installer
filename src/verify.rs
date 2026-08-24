use std::path::Path;

/// Validates that the installation environment meets all prerequisites
pub fn check_prerequisites() -> Result<(), String> {
    check_root_privileges()?;
    check_uefi_mode()?;
    
    // Additional checks like minimum RAM or CPU architecture can be added here
    Ok(())
}

fn check_root_privileges() -> Result<(), String> {
    // In Unix, the root user always has a UID of 0
    let uid = unsafe { libc::getuid() };
    if uid != 0 {
        return Err("The MITOS installer must be run as root (UID 0).".to_string());
    }
    Ok(())
}

fn check_uefi_mode() -> Result<(), String> {
    // The presence of this directory guarantees the system was booted via UEFI
    let efi_dir = Path::new("/sys/firmware/efi");
    if !efi_dir.exists() {
        return Err("System was not booted in UEFI mode. Legacy BIOS is not supported.".to_string());
    }
    Ok(())
}
