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
            // Added errors=remount-ro to prevent data corruption on disk errors
            fstab.push_str(&format!(
                "UUID={:<36}  /              ext4    defaults,noatime,errors=remount-ro  0       1\n",
                root_uuid
            ));
        }
        FilesystemType::Btrfs => {
            let btrfs_opts = "compress=zstd:1,noatime,discard=async";
            
            // Note: Using 'subvol=@' without the leading slash is the most 
            // universally compatible syntax across different kernel versions.
            fstab.push_str(&format!(
                "UUID={:<36}  /              btrfs   subvol=@,{}      0       0\n",
                root_uuid, btrfs_opts
            ));
            fstab.push_str(&format!(
                "UUID={:<36}  /home          btrfs   subvol=@home,{}      0       0\n",
                root_uuid, btrfs_opts
            ));
            
            // CRITICAL FIX: Added /var subvolume to match mount.rs behavior
            fstab.push_str(&format!(
                "UUID={:<36}  /var           btrfs   subvol=@var,{}      0       0\n",
                root_uuid, btrfs_opts
            ));
            fstab.push_str(&format!(
                "UUID={:<36}  /var/log       btrfs   subvol=@log,{}      0       0\n",
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
    // Sanitize hostname: systemd only allows alphanumeric and hyphens.
    // Stripping invalid chars prevents systemd-hostnamed from crashing on first boot.
    let clean_hostname: String = hostname
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
        
    let clean_hostname = if clean_hostname.is_empty() {
        "mitos".to_string() // Fallback if user entered only invalid chars
    } else {
        clean_hostname
    };
    
    // Write /etc/hostname
    let hostname_path = target_mount.join("etc/hostname");
    fs::write(&hostname_path, format!("{}\n", clean_hostname))
        .map_err(|e| format!("Failed to write /etc/hostname: {}", e))?;

    // Write /etc/hosts
    let hosts_content = format!(
        "# /etc/hosts\n\
         127.0.0.1       localhost\n\
         ::1             localhost ip6-localhost ip6-loopback\n\
         ff02::1         I'll ip6-allnodes\n\
         ff02::2         ip6-allrouters\n\
         127.0.1.1       {0}.localdomain {0}\n",
        clean_hostname
    );
    let hosts_path = target_mount.join("etc/hosts");
    fs::write(&hosts_path, hosts_content)
        .map_err(|e| format!("Failed to write /etc/hosts: {}", e))?;

    Ok(())
}

fn write_machine_id(target_mount: &Path) -> Result<(), String> {
    // Read directly from the kernel's random UUID generator. 
    // This avoids relying on the `uuidgen` binary being installed in the live environment.
    let machine_id = fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map(|s| s.replace("-", "").trim().to_lowercase())
        .unwrap_or_else(|_| "00000000000000000000000000000000".to_string());

    let machine_id_path = target_mount.join("etc/machine-id");
    fs::write(&machine_id_path, format!("{}\n", machine_id))
        .map_err(|e| format!("Failed to write /etc/machine-id: {}", e))?;

    // systemd and D-Bus require /etc/machine-id to be symlinked to /var/lib/dbus/machine-id
    let dbus_dir = target_mount.join("var/lib/dbus");
    fs::create_dir_all(&dbus_dir).unwrap_or_default();
    let dbus_machine_id = dbus_dir.join("machine-id");
    
    if dbus_machine_id.exists() || dbus_machine_id.is_symlink() {
        let _ = fs::remove_file(&dbus_machine_id);
    }
    
    // Create relative symlink: ../../../etc/machine-id
    std::os::unix::fs::symlink("../../../etc/machine-id", &dbus_machine_id)
        .map_err(|e| format!("Failed to symlink dbus machine-id: {}", e))?;

    Ok(())
}

/// Helper function to retrieve the UUID of a given partition using `blkid`
fn get_uuid(partition_path: &Path) -> Result<String, String> {
    // Use -p to probe the device directly, bypassing potentially stale udev/blkid caches 
    // immediately after partitioning.
    let output = Command::new("blkid")
        .args(["-p", "-s", "UUID", "-o", "value", partition_path.to_str().unwrap()])
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
