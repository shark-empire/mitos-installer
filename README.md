# mitos-installer

The system installer for **MITOS** — a Rust-based TUI (terminal UI) installer that
partitions a disk, deploys the base rootfs, configures the system, installs the Limine
bootloader, and can later repair or reconfigure an existing installation.

See [`INTEGRATION.md`](INTEGRATION.md) for how the installer fits into the wider MITOS
live-ISO pipeline (what it expects to find at `/run/mitos-live/`, which system tools it
shells out to, etc).

## Installer modes

```
mitos-installer [MODE] [--graphical | --text]

Modes (default: interactive install):
  --unattended <file>   Unattended install driven by a JSON answer file
  --recovery            Recovery mode for an existing MITOS installation

Theme (combinable with the default mode or --recovery; ignored by --unattended):
  --graphical           Full-color TUI (default)
  --text                Plain, minimal TUI - for serial consoles, etc.

  -h, --help            Show usage
```

**Graphical vs. text** are the same wizard, dressed differently — a colorful, styled
theme (arrow-key menus, highlighted prompts) versus a plain, linear theme with no ANSI
styling. Both walk through the same steps; "text" exists for serial consoles and other
terminals where styled redraws don't render reliably. This installer is a TUI end to end;
there's no separate pixel/framebuffer GUI. The theme flag is orthogonal to the mode, so
`mitos-installer --recovery --text` is valid.

**Recovery mode** does not create a new installation. It scans disks for an existing
MITOS system (identified via `/etc/os-release`), mounts it, and offers:

- **Repair boot** — regenerates the initramfs and reinstalls Limine (UEFI or BIOS,
  whichever the recovery media itself booted as).
- **Repair packages** — runs `mitos-pkg verify`/`reinstall` if present.
- **Reinstall system components** — re-extracts the base rootfs archive over the
  existing install (does not touch `/home`), then regenerates the initramfs and
  re-applies security policies.
- **Inspect logs** — shows the transaction log from the original install (copied onto
  the target disk during installation) plus the tail of the free-form install log.
- **Restore configuration** — re-writes hostname, locale, timezone, keyboard layout,
  and optionally resets network config to defaults.

## What gets installed

- **Partitioning**: GPT via `sgdisk`. Always an EFI System Partition; a small unformatted
  BIOS-boot partition is added automatically when installing under legacy BIOS; an
  optional dedicated swap partition (auto-sized from RAM, custom size, or none).
- **Filesystems**: ext4, or Btrfs with a `@ @home @var @log @snapshots` subvolume layout.
- **Encryption**: optional full-disk LUKS2 on the root partition (not swap/ESP). The
  passphrase is hashed/zeroed as soon as it's no longer needed; boot-time unlocking is
  wired through both the kernel command line (`rd.luks.*`, for the initramfs) and
  `/etc/crypttab` (for the booted system's own tooling).
- **Bootloader**: [Limine](https://github.com/limine-bootloader/limine), installed for
  whichever firmware mode (UEFI/BIOS) the live environment itself booted under. Detects
  an existing Windows EFI boot manager and adds a chainload entry automatically.
- **System config**: hostname, `/etc/hosts`, locale + timezone, console/X11 keyboard
  layout, `systemd-networkd`/`systemd-resolved` (+ `iwd` and a carried-over Wi-Fi profile
  if the live environment connected to one during setup), machine-id, `/etc/os-release`.
- **Users**: an initial admin user with sudo (`wheel`) access; supplementary groups
  (`video`, `audio`, `input`, ...) are added only if the target rootfs actually defines
  them.
- **Software**: a desktop/server choice (`systemctl set-default graphical.target` vs.
  `multi-user.target`, plus enabling/disabling a `mitos-session`/`mitos-gui` unit if
  present) and an optional extra package bundle (Minimal/Standard/Gaming/Creator) via
  `mitos-pkg`, best-effort since it needs live-environment network access.
- **Safety**: prerequisite checks (root privileges + every external tool the chosen
  options will need) before anything destructive happens; disk-capacity validation; a
  disk-path confirmation you have to *type*, not just arrow-select; the live boot medium
  itself is never offered as an install target; a structured, timestamped transaction log
  in addition to the free-form debug log; and an emergency-rollback path that deactivates
  swap, force-unmounts, closes any LUKS mapping, and wipes the partition table on failure
  so a retry starts from a clean disk.

## Unattended installs

`--unattended <path-to-answers.json>` skips every prompt. See
[`answerfile.rs`](src/answerfile.rs) for the authoritative field list; the shape is:

```jsonc
{
  "confirm_destructive": true,      // required - an explicit "yes, erase this disk"
  "language": "en-US",
  "disk": "/dev/sda",
  "filesystem": "ext4",             // "ext4" | "btrfs"
  "swap_auto": true,                // size swap from detected RAM
  "swap_mib": 0,                    // or set an explicit size (swap_auto must be false)
  "encrypt": false,
  "encryption_passphrase": null,    // required if "encrypt" is true
  "hostname": "mitos-pc",
  "username": "alex",
  "password": "changeme",           // plaintext in the file; hashed before use, never
                                     // written back out in plaintext anywhere
  "timezone": "America/New_York",
  "locale": "en_US.UTF-8",
  "keyboard_layout": "us",
  "install_profile": "standard",    // "minimal" | "standard" | "gaming" | "creator"
  "desktop": "graphical",           // "graphical" | "server"
  "wifi_ssid": null,
  "wifi_password": null
}
```

The requested disk is cross-checked against the same "available, writable, not the live
boot medium" list the interactive UI uses, so a stale or wrong answer file can't silently
target the wrong disk.

## Building

```
cargo build --release
```

Requires network access to fetch crates the first time (`dialoguer`, `indicatif`, `serde`,
`serde_json`, `thiserror`, `libc`, `nix`, `log`, `simplelog`, `rust-i18n`; see
`Cargo.toml`). `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D
warnings` mirror what CI runs.

The installer needs to run as root, and needs the external tools listed in
[`verify.rs`](src/verify.rs) available on `$PATH` (all standard on a Linux live
environment: `sgdisk`, `mkfs.*`, `cryptsetup`, `limine`, `efibootmgr`, `blkid`, `chroot`,
`useradd`/`chpasswd`, ...).
