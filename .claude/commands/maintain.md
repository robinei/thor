---
description: Routine maintenance of the AYN Thor - news, updates, the boot, the firmware, pacnew, health, notes
model: opus
---

Run a maintenance pass on this device (the AYN Thor: Arch Linux ARM, with the Thor's own packages from
the thor repository). The user started it from the dock's Maintenance app (`thor-maintain`): you run in
a terminal on the Thor's main screen, in `~/.local/share/thor/maintenance`, and the user types on the
bottom screen's keyboard, so keep messages short and ask few questions. `sudo` works without asking: the
Maintenance app took the password before starting you (if a `sudo` asks or fails, say so and stop
rather than retrying).

What is the Thor's rather than Arch's, all of it pacman's (`[thor]` in `/etc/pacman.conf`, from
github.com/robinei/thor's releases, signed by the key in `thor-keyring`; sources and docs in that
repository):
- `linux-thor`: Armada's kernel, its modules and the SM8550 device trees in `/usr/lib/modules/<version>`.
- `thor-system`: the boot (`thor-boot` puts the kernel, our initramfs and the command line on the card's
  FAT partition, both ways the card boots: systemd-boot's files and `/KERNEL` for the ABL; a pacman hook
  runs it after updates of the kernel, systemd, mkinitcpio and the rest the initramfs is made of), the
  power group's access, zram, sleep, the checks at boot (`thor-selftest`), the account helper.
- `thor-session`: the autologin into ClamShell (`clamshell.service`), the session's configs, Steam's
  starter, this playbook.
- `clamshell`: ClamShell and the fan daemon.
- `thor-ucm`: the Thor's sound profiles.
- `thor-sensors`: the motion sensors' services (hexagonrpcd), for ClamShell's gyro.
- `thor-firmware`: the Thor's firmware from ROCKNIX's release, and the motion sensors' configuration
  from PB-OS's repository, built on this device (never published: AYN's and Qualcomm's), so not updated
  by pacman from the repository: step 5.

Your notes are `changelog.md` in this folder (the device's own).

1. **Arch news.** `curl -s https://archlinux.org/feeds/news/` (Arch Linux ARM follows Arch's packages).
   For items newer than the last entry in `changelog.md` that mention manual intervention or a package
   installed here (`pacman -Qq`), summarize what to do and do it (or ask) before upgrading.

2. **Packages.** `checkupdates`: summarize, noting the Thor's own (above), systemd, mkinitcpio, mesa,
   pipewire, wireplumber, alsa-ucm-conf, bluez, networkmanager, upower. Ask before updating if a
   game might be running (ClamShell shows it; `systemctl --user list-units 'clamshell-run-*'`). Then
   `sudo pacman -Syu --noconfirm`. Read the output for warnings, failed hooks and `.pacnew` notices. If
   thor-boot ran, it must end with "thor-boot: done": if it failed, the card still boots the previous
   kernel (thor-boot replaces the card's files only once all the new ones are written and checked, and
   the previous kernel's modules are kept: thor-modules), but fix it before the user reboots (its log:
   `/var/log/thor-mkinitcpio.log`). If the kernel, systemd, mesa or the boot changed,
   tell the user a reboot is due (don't reboot yourself). If `clamshell` or `thor-session` changed, say
   that restarting ClamShell (`systemctl --user restart clamshell`) takes the new one, and that it closes
   every app and game with a window, this terminal too (ClamShell is their compositor): it is the user's
   to do, when nothing is running.
   If `thor-sensors` changed: `sudo systemctl restart thor-sensors` (its hexagonrpcd runs on as it was).
   Steam, if set up (`~/.local/share/Steam/steamrtarm64/steam`) and not running (`pgrep -x steam`):
   `steam-setup --update` (it starts without updating itself).

3. **pacnew/pacsave.** `pacdiff -o`. For each, diff against the live file, merge sensibly (keep this
   device's changes, such as `CriticalPowerAction=Hibernate` in `/etc/UPower/UPower.conf`; take
   upstream's new defaults), install the result, delete the .pacnew.

4. **The checks at boot.** `journalctl -b -t thor-selftest`: its `FAIL` lines (and new ones since the
   last pass, in `/var/log/thor-selftest.log`), explained to the user.

5. **Firmware.** If `/usr/share/thor/sensors/config` is missing (a thor-firmware from before the motion
   sensors' configuration: the checks at boot then fail thor-sensors), rebuild thor-firmware now, as
   below, from the repository's current PKGBUILD. Then compare `/usr/lib/firmware/thor-rocknix-release`
   with ROCKNIX's latest release (`curl -s https://api.github.com/repos/ROCKNIX/distribution/releases/latest`, the asset
   `ROCKNIX-SM8550.aarch64-*.tar`). Say what is newer (their changelog for SM8550 and the Thor); don't
   update unless the user asks. Updating, with the user: `thor-firmware`'s PKGBUILD (in a checkout of
   github.com/robinei/thor, `packages/thor-firmware`) with the new release's version and sha256 (its
   `.sha256` asset; the second checksum is PB-OS's archive's, unchanged), built here with `makepkg` (it
   downloads 1.5 GB, and PB-OS's 88 MB archive) and installed with `sudo pacman -U`;
   then reboot, and check screens, sound, Wi-Fi, Bluetooth and sleep.

6. **Health.** Orphans: `pacman -Qdtq` (list, ask before removing). Disk: `df -h /`, `du -sh ~/.cache/*`.
   Failed units: `systemctl --failed`, `systemctl --user --failed`. Recent errors: `journalctl -b -p err`.
   Swap and hibernate: `swapon --show` lists zram and `/swapfile`, and the loader entry's options have
   `resume=` and `resume_offset=`.

7. **Record.** Append a dated entry to `changelog.md` (what was updated, anything notable), and the
   device's own experiments still in place, deliberate, for later passes: `/etc/systemd/sleep.conf.d/`
   (deep sleep), `~/.config/environment.d/` (`CLAMSHELL_CHARGE_LIMIT`), `/etc/modprobe.d/`
   (`rsinput.mcu_frame`). Summarize for
   the user, with anything that needs them (a reboot, a check after a new kernel or firmware).
