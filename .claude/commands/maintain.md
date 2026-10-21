---
description: Routine maintenance of the AYN Thor - news, updates, the boot images, ClamShell, ROCKNIX, pacnew, health, changelog
model: opus
---

Run a maintenance pass on this device (the AYN Thor, Arch Linux ARM with Armada's kernel, ROCKNIX's
firmware and our session). The user started it from the dock's Maintenance app (`tools/device/maintain`): you run in a
terminal on the Thor's main screen and the user types on the bottom screen's keyboard, so keep messages
short and ask few questions. `sudo` needs no password here.

What is ours rather than Arch's (see `tools/image/thor-image.sh`, which builds the image, and
`docs/thor-gameplan.md`, "Our image"):
- The kernel, its modules and most firmware are ROCKNIX's SM8550 release (no Arch kernel package is
  installed): `/usr/lib/modules/<version>`, `/usr/lib/firmware`, the release named in
  `/usr/local/share/thor/rocknix-release`. If `/usr/local/share/thor/kernel` exists, the kernel and its
  modules are instead our build of Armada's kernel (`tools/image/thor-kernel.sh`, built on a PC; the
  file says from what); the firmware is still ROCKNIX's.
- Our initramfs (`/etc/mkinitcpio-thor.conf`) and the two ways the card boots, both on its FAT partition:
  systemd-boot with the kernel as files (`/Image`, `/dtbs/`, `/initramfs-thor.img`, the command line in
  `/loader/entries/thor.conf`), and `/KERNEL`, an Android boot image for ROCKNIX's ABL. `thor-boot`
  (`tools/device/thor-boot`) rebuilds the initramfs and puts it and the command line into both; a pacman
  hook runs it after updates of systemd, mkinitcpio, kmod, util-linux or zstd.
- ROCKNIX's audio profiles over alsa-ucm-conf's (a pacman hook puts them back after its updates, from
  `/usr/local/share/thor/ucm2`).
- The session: ClamShell, built from `clamshell/` in this repository, and its files (sway, mako, foot,
  the service, the dock's apps) written by `tools/device/session.sh`.
- The system files, written by `tools/device/system.sh`: the fan daemon `clamshell-fan` (built with
  ClamShell; a system service), the `power` group's udev rule and tmpfiles entry, the sticks' range in
  modprobe.d, zram's config and the swapping sysctls, the SD card's scheduler rule, PipeWire's buffers,
  the checks at boot (`thor-selftest`: `journalctl -b -t thor-selftest`; its `FAIL` lines are worth a
  look in every pass), the wake log hook, Steam's starter (`/usr/local/bin/steam`, `steam-setup`).

1. **Prepare.** `git status` in this repository. Uncommitted changes: show them and ask whether to
   commit them first. `git pull --rebase` (if it fails for lack of access, say so: the device's ssh key
   isn't on GitHub yet, `~/.ssh/id_ed25519.pub`; make one with `ssh-keygen -t ed25519` if missing, and
   go on without pulling).

2. **Arch news.** `curl -s https://archlinux.org/feeds/news/` (Arch Linux ARM follows Arch's packages).
   For items newer than the last entry in `docs/changelog.md` that mention manual intervention or a
   package installed here (`pacman -Qq`), summarize what to do and do it (or ask) before upgrading.

3. **Packages.** `checkupdates`: summarize, noting systemd, mkinitcpio, mesa, sway, pipewire, wireplumber,
   alsa-ucm-conf, bluez, networkmanager, upower. Ask before updating if a game might be running (ClamShell
   shows it; `systemctl --user list-units 'clamshell-game-*'`). Then `sudo pacman -Syu --noconfirm`. Read
   the output for warnings, failed hooks and `.pacnew` notices. If thor-boot ran, it must end with
   "thor-boot: done": if it failed, the card still boots the previous images (they are only replaced
   once the new ones are made and checked), but fix it before the user reboots (its log:
   `/var/log/thor-mkinitcpio.log`). If systemd, mesa or the boot images changed, tell the user a reboot is
   due (don't reboot yourself).
   Steam, if set up (`~/.local/share/Steam/steamrtarm64/steam`) and not running (`pgrep -x steam`):
   `steam-setup --update` (it starts without updating itself).

4. **ClamShell and the session.** If the pull brought changes in `clamshell/` or `tools/device/`, or
   `~/.local/share/thor/session-commit` isn't the current commit: `cd clamshell && cargo build --release`,
   then `tools/device/session.sh ~ ~ clamshell/target/release/clamshell`, `systemctl --user daemon-reload`,
   `sudo tools/device/system.sh / clamshell/target/release/clamshell-fan`, `sudo systemctl daemon-reload`,
   `sudo systemctl restart clamshell-fan`, `sudo udevadm control --reload`, `sudo sysctl --system` (the
   udev rules, the modprobe options, zram's config and PipeWire's buffers take effect at the next boot), and write `git rev-parse HEAD` to `~/.local/share/thor/session-commit`. Restarting ClamShell
   (`systemctl --user restart clamshell`) redraws both screens; apps and games keep running (each in a
   scope of its own, this terminal too): ask first. sway's config changes take effect at the
   next login (or `swaymsg reload`, after which ClamShell must be restarted: a reload drops the rules it
   gave sway).

5. **ROCKNIX.** Compare `/usr/local/share/thor/rocknix-release` with the latest release
   (`curl -s https://api.github.com/repos/ROCKNIX/distribution/releases/latest`, the asset
   `ROCKNIX-SM8550.aarch64-*.tar`). Say what is newer (their changelog for SM8550 and the Thor); don't
   update unless the user asks. Updating, with the user: download and check the sha256, unpack
   `target/KERNEL` and `target/SYSTEM`, and do what `thor-image.sh` does for them (its "ROCKNIX's system",
   "the kernel's modules, ROCKNIX's firmware", "ROCKNIX's audio profiles" and boot image steps): the new modules in
   `/usr/lib/modules` (the old directory removed, so there is one), firmware, the profiles (into
   `/usr/share/alsa/ucm2` and `/usr/local/share/thor/ucm2`), `bootimg.py split` for `/Image` and the
   device tree, `/KERNEL` replaced by the new release's, then `thor-boot` (which puts our initramfs and
   command line into it), and the release name recorded. Keep the old files on the FAT partition
   (`KERNEL.old`, `Image.old`) until the user has booted the new kernel and checked screens, sound, Wi-Fi
   and sleep. With our kernel (`/usr/local/share/thor/kernel`), take only the firmware and profiles from
   the release and keep the kernel, modules and `/KERNEL`: putting in ROCKNIX's kernel would drop binder
   (Waydroid) and Armada's patches. A newer kernel of ours is built on a PC (`tools/image/thor-kernel.sh`,
   Armada's latest or `ARMADA_REF`) and copied over.

6. **pacnew/pacsave.** `pacdiff -o`. For each, diff against the live file, merge sensibly (keep our
   changes, such as `CriticalPowerAction=Hibernate` in `/etc/UPower/UPower.conf`; take upstream's new
   defaults), install the result, delete the .pacnew.

7. **Health.** Orphans: `pacman -Qdtq` (list, ask before removing). Disk: `df -h /`, `du -sh ~/.cache/*`,
   `clamshell/target` (`cargo clean` if large). Failed units: `systemctl --failed`,
   `systemctl --user --failed`. Recent errors: `journalctl -b -p err`. Swap and hibernate:
   `swapon --show` lists `/swapfile`, and the loader entry's options have `resume=` and `resume_offset=`.

8. **Record.** Append a dated entry to `docs/changelog.md` (what was updated, anything notable), update
   other docs if facts changed, commit, and `git push` (if access works). Summarize for the user, with
   anything that needs them (a reboot, a check after a new kernel).
