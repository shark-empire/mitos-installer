# MITOS Installer Integration

This is the interactive TUI (Terminal User Interface) system installer for MITOS. It runs in the Live ISO environment before the first boot.

## The Installation Pipeline

This installer is responsible for taking a blank disk and turning it into a bootable MITOS system. It connects to the rest of the ecosystem through the following pipeline:

1. **Hardware & Network:** Probes disks and optionally connects to Wi-Fi via `nmcli`.
2. **Partitioning & Formatting:** Wipes the target disk and formats it as either Ext4 or Btrfs (with subvolumes).
3. **Rootfs Extraction:** Mounts the target disk and extracts the pre-compiled MITOS root filesystem (containing `mitos-init`, `mitos-services`, `mitos-session`, `mitos-gui`, etc.).
4. **Bootloader Installation:** Installs and configures the **Limine** bootloader on the EFI partition, including Windows dual-boot detection.
5. **System Configuration:** Writes the user's chosen hostname, timezone, and locale to `/etc/hostname`, `/etc/timezone`, and `/etc/locale.conf`.
6. **User Creation & Security:** Creates the initial admin user. Crucially, it hashes the user's password via `openssl passwd -6` and uses `chpasswd -e` to inject the SHA-512 hash directly into `/etc/shadow`. 
7. **Init Handoff:** Configures `mitos-init` via `init.rs` so that when the system reboots, PID 1 knows how to launch `mitos-services`.

## How it connects to `mitos-session`
Because this installer securely writes the SHA-512 password hash to `/etc/shadow` during installation, the newly installed system is immediately ready for `mitos-session`'s lock screen logic. When the user logs in for the first time and presses `Super+L`, `mitos-session` reads `/etc/shadow`, sees a valid hash, and successfully triggers the lock screen without requiring the user to manually set a password first.

## Emergency Recovery
If any step in the pipeline fails (e.g., a bad sector during formatting), the `recovery::trigger_emergency_cleanup` function unmounts the target disk and wipes partial data to leave the hardware in a clean state for a retry.
