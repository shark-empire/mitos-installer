use std::fs;
use std::path::Path;
use std::process::Command;

/// Define supported filesystems so we can generate the correct mount options
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilesystemType {
    Ext4,
    Btrfs,
}

/// Generates core system files: fstab, hostname, hosts, and machine-id
pub fn configure_system(
    target_mount: &Path,
    root_partition: &Path,
    efi_partition: &Path,
    hostname: &str,
    fs_type: FilesystemType,
) -> Result<(), String> {
    let etc_dir = target_mount.join("etc");
    fs::create_dir_all(&etc_dir).map_err(|e| format!("Failed to create /etc directory: {}", e))?;

    write_fstab(target_mount, root_partition, efi_partition, fs_type)?;
    write_hostname_and_hosts(target_mount, hostname)?;
    write_machine_id(target_mount)?;
    
    Ok(())
}

fn write_fstab(
    target_mount: &Path, 
    root_part: &Path, 
    efi_part: &Path, 
    fs_type: FilesystemType
) -> Result<(), String> {
    let root_uuid = get_uuid(root_part)?;
    let efi_uuid = get_uuid(efi_part)?;

    let mut fstab = String::from(
        "# /etc/fstab: static file system information.\n\
         # <file system>                           <mount point>  <type>  <options>                   <dump>  <pass>\n"
    );

    // 1. EFI Partition (umask=0077 restricts access to root for security)
    fstab.push_str(&format!(
        "UUID={:<36}  /boot/efi      vfat    umask=0077                  0       2\n",
        efi_uuid
    ));

    // 2. Root Partition (Dynamically formatted based on chosen Filesystem)
    match fs_type {
        FilesystemType::Ext4 => {
            fstab.push_str(&format!(
                "UUID={:<36}  /              ext4    defaults,noatime            0       1\n",
                root_uuid
            ));
        }
        FilesystemType::Btrfs => {
            // Modern Btrfs standard uses subvolumes (@ for root, @home for /home, etc.)
            let btrfs_opts = "compress=zstd:1,noatime,discard=async";
            
            fstab.push_str(&format!(
                "UUID={:<36}  /              btrfs   subvol=/@,{}      0       0\n",
                root_uuid, btrfs_opts
            ));
            fstab.push_str(&format!(
                "UUID={:<36}  /home          btrfs   subvol=/@home,{}      0       0\n",
                root_uuid, btrfs_opts
            ));
            fstab.push_str(&format!(
                "UUID={:<36}  /var/log       btrfs   subvol=/@log,{}      0       0\n",
                root_uuid, btrfs_opts
            ));
        }
    }

    // 3. tmpfs for /tmp (Standard practice for performance and SSD longevity)
    fstab.push_str("tmpfs                                  /tmp           tmpfs   defaults,noatime,mode=1777  0       0\n");

    let fstab_path = target_mount.join("etc/fstab");
    fs::write(&fstab_path, fstab)
        .map_err(|e| format!("Failed to write /etc/fstab: {}", e))?;

    Ok(())
}

fn write_hostname_and_hosts(target_mount: &Path, hostname: &str) -> Result<(), String> {
    let clean_hostname = hostname.trim();
    
    // Write /etc/hostname
    let hostname_path = target_mount.join("etc/hostname");
    fs::write(&hostname_path, format!("{}\n", clean_hostname))
        .map_err(|e| format!("Failed to write /etc/hostname: {}", e))?;

    // Write /etc/hosts (Crucial: prevents "sudo: unable to resolve host" errors)
    let hosts_content = format!(
        "# /etc/hosts\n\
         127.0.0.1       localhost\n\
         ::1             localhost\n\
         127.0.1.1       {}.localdomain {}\n",
        clean_hostname, clean_hostname
    );
    let hosts_path = target_mount.join("etc/hosts");
    fs::write(&hosts_path, hosts_content)
        .map_err(|e| format!("Failed to write /etc/hosts: {}", e))?;

    Ok(())
}

fn write_machine_id(target_mount: &Path) -> Result<(), String> {
    // systemd requires a unique 128-bit machine-id. 
    // We generate it now so the installed system doesn't share an ID with the Live USB.
    let output = Command::new("uuidgen")
        .output()
        .map_err(|e| format!("Failed to execute uuidgen: {}", e))?;

    let machine_id = if output.status.success() {
        // uuidgen outputs standard UUID format, we strip dashes and lowercase it
        String::from_utf8_lossy(&output.stdout)
            .replace("-", "")
            .trim()
            .to_lowercase()
    } else {
        // Fallback if uuidgen fails
        "00000000000000000000000000000000".to_string()
    };

    let machine_id_path = target_mount.join("etc/machine-id");
    fs::write(&machine_id_path, format!("{}\n", machine_id))
        .map_err(|e| format!("Failed to write /etc/machine-id: {}", e))?;

    Ok(())
}

/// Helper function to retrieve the UUID of a given partition using `blkid`
fn get_uuid(partition_path: &Path) -> Result<String, String> {
    let output = Command::new("blkid")
        .args(["-s", "UUID", "-o", "value", partition_path.to_str().unwrap()])
        .output()
        .map_err(|e| format!("Failed to execute blkid: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to retrieve UUID for {:?}: {}",
            partition_path,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let uuid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uuid.is_empty() {
        return Err(format!("blkid returned empty UUID for {:?}", partition_path));
    }

    Ok(uuid)
}
