# Game plan: when the AYN Thor arrives

The device: AYN Thor Max (16 GB / 1 TB, Snapdragon 8 Gen 2 / SM8550, 6" top + 3.92" bottom OLED).
The goal: our own minimal Arch Linux ARM with sway and ClamShell, booted from SD card first,
Android kept. Written 2026-10-14, before having the device; everything here is unverified on it.

## How the boot works

- **Stock: U-Boot with UEFI.** Kettle Linux's Thor notes: the Thor ships with U-Boot offering UEFI; they
  boot GRUB as an EFI program, which loads the device tree `qcs8550-ayn-thor.dtb`; SD card boot works
  through U-Boot's EFI. (ROCKNIX's Thor page agrees there is a stock way: its ABL steps "are only required
  if your device doesn't have a fastboot menu to switch the Boot Mode".) Verify on the device that ours
  has it (day one, step 2) before flashing anything.
- **Else ROCKNIX's ABL** (`3rdparty/bootloader/rocknix_abl/abl_signed-SM8550.elf` in their release, with
  backup, flash and restore scripts): finds a FAT partition labelled `ROCKNIX` on internal storage, SD or
  USB and boots its `/KERNEL`; volume up at power-on picks the target, Android included. ROCKNIX's own
  SM8550 build uses this (`BOOTLOADER="qcom-abl"`).
- `/KERNEL` (ROCKNIX 20261001) is an Android boot image, header version 0, page size 2048: a gzip'd arm64
  `Image` with 14 device trees appended (AYN Thor, Odin 2 / Mini / Portal, gamepad nodes; the ABL picks
  the matching one), the initramfs built into the kernel (the ramdisk is 5 bytes), and the command line
  `boot=LABEL=ROCKNIX disk=LABEL=STORAGE quiet rootwait console=tty0 allow_mismatched_32bit_el0
  fw_devlink.strict=1 pcie_ports=compat irqaffinity=0-2 cgroup.memory=nokmem,nosocket nosoftlockup
  usbcore.interrupt_interval_override=045e:028e:2`. The kernel is built with the EFI stub (it starts
  with `MZ` and a PE header), so the same kernel boots through UEFI too; its Thor device tree is
  `ayn,thor` / `qcom,qcs8550` / `qcom,sm8550`.

## The hardware, as Kettle Linux found it

- Screens: top DSI-2, bottom DSI-1, **both natively portrait (1080x1920)**: sway needs a transform on each.
- Touch: the device tree swaps and inverts axes; Kettle fixes it with `LIBINPUT_CALIBRATION_MATRIX` in a
  udev rule (`61-kettle-thor-touch.rules`), and maps each touchscreen to its screen.
- Audio: card `AYN-Thor` (`ayn-AYNOdin2` under EFI: U-Boot reports the Odin 2's SMBIOS); UCM profiles
  (their `kettle-ucm-ayn`) and firmware `ayn/thor/*` incl. `AYN-Thor-tplg.bin`; internal mic on DMIC3.
- Gamepad: InputPlumber (their `40-kettle-thor.yaml`); the AYN key as quick access.
- Power: 4-18 W; at 5 W the three CPU clusters cap at 1344/1785/1977 MHz. Fan curve in the device tree.
- Lid switch: sleep on close (`wakeup-event-action`); suspend (s2idle) verified.
- Backlights: the top one first (`ae96000.dsi.0`), the bottom labelled `bottom-panel`.
- The bottom screen draws about 0.7 W at idle. Wi-Fi and Bluetooth work.

## Our image (`tools/vm/vm.py --arch aarch64 thor-image`, built in the ARM virtual machine)

What goes in, and from where:

| Part | Source |
|---|---|
| Bootloader | not in the image: stock U-Boot (UEFI), else ROCKNIX's ABL flashed once (see below) |
| UEFI boot, on the same FAT partition (an EFI system partition) | systemd-boot as `EFI/BOOT/BOOTAA64.EFI`, ROCKNIX's kernel as `/Image` (decompressed), the Thor's device tree as `/dtbs/qcs8550-ayn-thor.dtb`, our initramfs, a loader entry with our command line; split out by `tools/image/bootimg.py split` |
| `/KERNEL` on FAT `ROCKNIX` (GPT name `system`, legacy-boot attribute, at 16 MiB) | ROCKNIX's kernel and device trees, unchanged; our initramfs (mkinitcpio: base udev modconf block thor-root-wait resume filesystems fsck, no autodetect) and command line (`root=LABEL=THOR_ROOT rw` + ROCKNIX's SM8550 parameters; the first boot adds `resume=`, `resume_offset=`), put into both boot ways by `thor-boot` (`tools/device/thor-boot`, repacking with `tools/image/bootimg.py`), as the image is built and again on the device after updates |
| Kernel modules, firmware | ROCKNIX's `SYSTEM` (same release), incl. Qualcomm firmware: for this device only, not for sharing |
| Root: ext4 `THOR_ROOT` | `pacstrap` from Arch Linux ARM's mirrors (unpinned): base, sudo, mkinitcpio, NetworkManager, OpenSSH, sway, mako, foot, Xwayland, Mesa + `vulkan-freedreno`, PipeWire, Noto, SQLite, Python, vim, grim, growpart (`cloud-guest-utils`), gptfdisk; RetroArch with cores for NES, SNES, N64, Game Boy, GBA, DS, Mega Drive, PlayStation and Dreamcast and XMB's assets (its menu in ClamShell's colours: `clamshell/retroarch/menu-theme.py`, needing Pillow), mpv, ffmpeg; for maintenance on the device git, Rust, base-devel, pacman-contrib |
| System | time zone (`THOR_TIMEZONE`, else the build machine's `/etc/localtime`, else UTC), locale (`THOR_LOCALE`, default `en_US.UTF-8`), systemd-timesyncd, NetworkManager, sshd, Bluetooth |
| Session | user `thor` (groups wheel, video, input, audio; passwordless sudo, my ssh key), tty1 autologin into sway; the session's files from `tools/device/session.sh` (also run by maintenance): sway config (DSI-2 top, DSI-1 bottom, ROCKNIX's output lines; Print: `clamshell ctl screenshot`; `clamshell/sway/clamshell.conf` included), mako's and foot's configs, the Maintenance app's desktop file, the dock's first pins (Steam, foot, Maintenance), the starter `library.json` (`tools/device/library.json`: Games with a folder per system in `~/Games/<system>`, Films and TV in `~/Videos`) unless there is one; ClamShell built natively for aarch64 as a user service (control pipe `$XDG_RUNTIME_DIR/clamshell-control`, `CLAMSHELL_SESSION=device`: it puts the device to sleep when idle and reads the volume and brightness keys itself), uinput rule |
| Maintenance | this repository cloned to `~/thor` (origin `git@github.com:robinei/thor.git`); the Maintenance app (a desktop file, pinned to the dock) runs `tools/device/maintain`: Claude Code with `/maintain` (`.claude/commands/maintain.md`). pacman hooks: `thor-boot` after updates of systemd, mkinitcpio, kmod, util-linux, zstd; ROCKNIX's audio profiles put back after alsa-ucm-conf's (a copy in `/usr/local/share/thor/ucm2`). The ROCKNIX release in `/usr/local/share/thor/rocknix-release` |
| Sleep | logind: power button and lid do `sleep` = suspend-then-hibernate, else suspend (`SleepOperation=`); long press powers off. `HibernateDelaySec=30min`. UPower: hibernate at 3 % (power off if it can't) |
| First boot (`thor-firstboot.service`, once) | grows partition 2 and its ext4 to the card (growpart, resize2fs); makes `/swapfile`, 16 GiB (the RAM; only if 8 GiB stay free beside it), in fstab; then writes `resume=LABEL=THOR_ROOT resume_offset=<first block>` into the loader entry's options and runs `thor-boot`, which puts that command line into `/KERNEL` too. Resume works from the second boot on |

Built once (2026-10-14, ROCKNIX 20261001, kernel 7.2.0): `~/.cache/thor-vm/aarch64/thor-sd.img`, 12 GB
(2.5 GB used). Checked from the host and in the build machine: GPT with partition 1 an EFI system
partition (GPT name `system`, legacy-boot attribute, FAT label `ROCKNIX`) holding `KERNEL`,
`EFI/BOOT/BOOTAA64.EFI`, `Image`, `dtbs/qcs8550-ayn-thor.dtb`, `initramfs-thor.img`, `loader/`; partition
2 `THOR_ROOT` with ROCKNIX's modules (7.2.0), its firmware (161 MB: `qcom/sm8550/a740_zap.mbn` for the
GPU, `AYN-Thor-tplg.bin` for audio, `ayn/`), its UCM profiles (`AYN/Thor`), no Arch kernel package, the
session and the program. Not booted anywhere: the first boot is on the device.

What ROCKNIX's system showed (used by the build):

- Modules and part of the firmware are in a kernel overlay (`usr/lib/kernel-overlays/base/lib/...`);
  `usr/lib/firmware` is a link to `/run/kernel-overlays/firmware`, set up at run time.
- Its sway outputs are written at session start by `usr/lib/autostart/common/111-sway-init`: the
  transform from `/sys/class/graphics/fbcon/rotate` (1/2/3: 90/180/270), and for the Thor the
  touchscreens `0:0:top_touchscreen` on DSI-2 and `0:0:bottom_touchscreen` on DSI-1 (bottom one's events
  off unless something shows there). Our image does the same (`~/.local/bin/thor-outputs`, rotating both
  panels by fbcon's angle).

Known issues and risks, to check first:

1. Our ramdisk is meant to be unpacked over the kernel's built-in initramfs, so our `/init` wins. If not,
   ROCKNIX's init runs, looks for its own `SYSTEM`, and the boot fails (ABL path only; the UEFI path
   passes our initramfs as the only one... unless the built-in one still unpacks first: same question).
2. Early storage: settled. sdhci-msm is built into ROCKNIX's kernel (its messages are in the Image), as
   are ext4 and f2fs.
3. Audio: ROCKNIX's UCM profiles are copied over `alsa-ucm-conf`'s (a pacman hook puts them back after
   an update of that package); under UEFI the card is named `ayn-AYNOdin2` (ROCKNIX has a profile for that name too).
4. Rotation: the bottom panel gets fbcon's angle as well; unverified that it wants the same. Touch may
   still need Kettle's calibration matrix.
5. Gamepad: the kernel driver comes with ROCKNIX's modules; its userspace mapping (Thorch, Kettle:
   InputPlumber maps) doesn't. Check what evdev reports before relying on it.
6. Device trees: Thorch insists on exactly one Thor device tree in `/KERNEL`; we keep ROCKNIX's 14 (as
   ROCKNIX does). The UEFI path loads only the Thor's.
7. UEFI path: whether U-Boot passes our device tree through systemd-boot (its devicetree fixup) or
   insists on its own; Kettle uses GRUB, the fallback.
8. Hibernate, unverified: ROCKNIX's kernel has it (`CONFIG_HIBERNATION=y`, LZO compression; checked
   2026-10-15 in its embedded config). Not known: whether the Thor's firmware and drivers come back
   after a resume (the remote processors for audio, sensors, Wi-Fi; the GPU's zap shader; the panels);
   whether the ABL or U-Boot path matters; whether the RTC alarm wakes the Thor from suspend (without
   it suspend-then-hibernate never gets to hibernate); whether the first boot's growpart, swap file and
   command line rewrite work (never run: the image hasn't been booted). If the card is not up when
   the initramfs's resume runs (thor-root-wait gives it 10 s) the hibernated state is lost and the
   machine boots fresh.
9. Not done: fan control. ClamShell applies its settings again on waking.
10. Test image only: passwordless sudo, sshd with a key, autologin without a password.

## Before the device arrives

- [x] Build the image and check it (done 2026-10-14; see above).
- [x] Settle 2: sdhci-msm is built in (checked 2026-10-14, strings in the Image; f2fs too).
- [ ] Rebuild nearer the time: a newer ROCKNIX release, current packages.
- [ ] Find out what the stock Linux boot mode is (U-Boot? AYN's own ABL?), what it reads (`/KERNEL`, or
      `extlinux.conf` and files), and from which firmware version it ships; ask on the Discord.
- [ ] Read ROCKNIX's ABL scripts and README (flash, backup, restore) and the Thorch and Pocknix install
      notes; write down the exact flashing steps and the way back to stock.
- [ ] Have a fast SD card (A2/U3; the Thor's slot runs SDR104) and a USB-C keyboard and Ethernet adapter.
- [ ] Download a known-good image to compare against when ours fails: ROCKNIX SM8550, Pocknix
      (`pocknix-sm8550-*-sd.img.zst`) or Thorch.

## Day one, in order

1. Boot Android, update its firmware, note the versions (Android build, bootloader). Charge.
2. **Check for a stock Linux boot mode before flashing anything**: enter the fastboot menu (power and
   volume) and look for a Boot Mode switch (Android / Linux), or anything naming U-Boot. Note exactly what
   it offers and where it boots from. If it's there, skip step 4: no ROCKNIX ABL.
3. Back up first anyway: the stock ABL (ROCKNIX's `backup_abl.sh`) and anything else the install notes
   call out, copied off the device.
4. Only without a stock Linux mode: flash ROCKNIX's ABL (`flash_abl.sh`, as root in Android). Check that
   Android still boots (volume up at power-on, Android).
5. Boot a known-good image (ROCKNIX or Pocknix) from SD: proves the ABL, the SD slot and the screens.
   Note what ROCKNIX's session shows for outputs (`swaymsg -t get_outputs`), inputs
   (`swaymsg -t get_inputs`, `evtest`), the gamepad's device and buttons, sound devices, Wi-Fi.
6. Boot our image from SD. If it doesn't come up, the known issues above are the suspects, in order.
   Serial console: none known; a USB keyboard and the screen, or ssh over USB Ethernet.
7. Library: put games in `~/Games/<system>` and videos in `~/Videos/Films` and `~/Videos/TV` (a folder per
   series); add a TMDB key to `~/.config/clamshell/library.json` (`"keys": { "tmdb": "..." }`) for films' and
   series' posters and overviews; Tools → Rescan library.
8. Maintenance: install Claude Code (`curl -fsSL https://claude.ai/install.sh | bash`) and log in; make
   the device's ssh key (`ssh-keygen -t ed25519`) and add it to the GitHub repository (a deploy key with
   write access); then a first `/maintain` from the dock's Maintenance app.

## Testing our system on the device

Each subsystem, then hibernate with each in use (hibernate matters more to me than suspend).

- [ ] Screens: both on, right size, orientation and position; layer surface on DSI-1.
- [ ] Touch: the bottom screen's touch on DSI-1 (sway's `map_to_output`); two fingers on the bar at
      once (one sliding, one tapping); below the bar, a finger sliding over anything points (on a list
      only sideways; it fades; taps click, two fingers scroll) and taps act again a second after (or
      when a text field takes focus); the Touchpad tab points at once: how it feels, the threshold
      (`POINT_START`).
- [ ] Gamepad: evdev events; our virtual pad and pad mouse; per-game keymaps (a key chord, the pointer
      from a stick) and deadzone; the back buttons' key codes (ClamShell takes BTN_TRIGGER_HAPPY1-4 and
      BTN_GRIPL/R/L2/R2 as L4, R4, L5, R5); unplugging
      the controller mid-game (the game's pad stays, at rest, and picks up again); A held on an
      on-screen key while View moves the pad away (the key must not keep repeating).
- [ ] Back button (the small round one at the bottom screen's bottom-right corner): which device and
      key code it sends (`evtest`). ClamShell assumes KEY_BACK on the controller (`BACK_KEY` in
      `pad/remap`, and the fake pad's `back`); if it comes from a separate button device, the pad thread
      has to read (and grab) that one too. Then: Back as a keymap input in a game (mapped to a layer, a
      key), Back on ClamShell's screens goes back, and on the keymap edit screen it selects Back.
- [ ] Home then Back with Steam: the virtual pad's Guide (BTN_MODE) while both are held opens Steam's
      overlay, and Guide chords (Guide with a button pressed meanwhile) work; Home alone still goes to
      shelf, Home with R1 takes a screenshot.
- [ ] Gamepad buttons the keymap edit screen draws but can't remap yet: which codes the AYN button under
      the screen and the two small buttons beside it send (`evtest`), and from which device; the
      bottom-left round one should be Home (BTN_MODE). Also confirm the face buttons' codes by their printed letters (B: BTN_SOUTH, A:
      BTN_EAST, X: BTN_NORTH, Y: BTN_WEST: by place, as ClamShell reads them; an xpad controller's,
      by letter, are swapped as they come in) and that games see them as expected through the virtual
      pad (written by letter, as an Xbox 360 pad's), and that there are no back buttons. Then they can become
      inputs of their own in the keymap (`core::keymap::Button`) and the edit screen (`views/pad.rs`).
- [ ] Gyro: what the Thor's IMU is (an IIO device, or evdev from the gamepad driver?), its axes, signs and
      units. ClamShell reads an evdev motion sensor (INPUT_PROP_ACCELEROMETER, ABS_RX/RY/RZ, resolution in
      units per °/s) paired with the controller; an IIO sensor needs a source of its own behind the same
      seam (`pad/motion.rs`, `MotionSource`). Its offset at rest (°/s, and how it drifts warm): whether
      the still detection (under 5 °/s, within 0.5 °/s, for 1.5 s) catches it and doesn't eat slow aim,
      and "Calibrate gyro". Then the feel: gyro to the pointer at each speed (pixels per degree),
      acceleration and tightening's defaults, which way turning and tilting move it (`ROLL_TURNS` in
      `pad/remap.rs` for tilting), gyro to a stick with its minimum, holding a trigger to aim, and the
      ratchet.
- [ ] Rumble: a game's rumble on the controller's motors at each Rumble setting; the keymap's buzzes (a
      layer armed or toggled on, and off: `Buzz` in `pad/rumble.rs`, 40 ms at 40000 on the light motor,
      30000 on the heavy one) felt, and not confused with a game's.
- [ ] Drawing cost: how long a full redraw of each screen takes on the device's CPU (tiny-skia, no GPU)
      and what the second screen costs in power while a game runs (a slider held, a list flung, the
      keymap edit screen with its labels).
- [ ] GPU: Freedreno (sway) and Turnip (`vkcube`, a game); MangoHud frame rate in sysinfo.
- [ ] Direct scanout. The spec sheet gives the panels as 1080x1920 (main, 120 Hz) and 1080x1240 (second,
      60 Hz): natively portrait, mounted sideways, as our configs' transforms assume. Confirm with the
      connectors' native modes. Rotated, sway composites a fullscreen game every frame (wlroots rotates
      itself; it doesn't use a plane's rotation): measure what that pass costs at 120 Hz (GPU time,
      power), and see it in sway's debug log ("direct scan-out"). Whether the display engine's planes
      rotate RGB buffers by 90° (`drm_info`: each plane's rotation property and values): if so, gamescope
      (which uses it) could show games without a composition pass.
- [ ] HDR, an experiment (the panels take no HDR signal; HDR as on phones: the panel driven brighter,
      ordinary content drawn dimmer within it, highlights above). Peaks from the spec sheets: top 650
      nits, bottom 550. The bottom needs no HDR, but it shows dual-screen emulators' second screen (DS,
      3DS, Wii U), so its white and colours must match the top's: with the top in headroom mode, the
      bottom's ordinary white at the same level as the top's (about 200 nits), and the two panels'
      white points and gamuts matched to begin with (two OLED panels can differ). First the facts: the DSI link's bits per colour and
      whether the display processor dithers (banding); how far the kernel's brightness goes and whether
      its top is near 650 nits or the panel's high-brightness mode is out of reach; which colour
      properties msm exposes on the connector and CRTC (CTM, GAMMA_LUT, DEGAMMA_LUT); Turnip under
      wlroots' Vulkan renderer (Sway 1.12's HDR needs it). Then a game with HDR under gamescope on the
      top screen with HDR output forced (its debug options), the peak at about 650 nits and ordinary
      content at about 200 (about 1.7 stops of headroom), the panel at the top of its brightness: do the
      highlights look better, without banding, heat or burn-in worries (a cap or time limit on highlights
      if needed). Only if it pays: a headroom output mode patched into wlroots and Sway for the desktop.
- [ ] Wi-Fi from the Wi-Fi mode (NetworkManager): scan, join with a password, forget; polkit lets the
      session's user do it (tested in the VM as an autologin session user, not on the device).
- [ ] Bluetooth: does the chip work with ROCKNIX's kernel and firmware? Then the Bluetooth mode: pair
      headphones (sound moves to them) and a controller (no code needed), forget one.
- [ ] Text fields bring up the keyboard tab (input-method-v2 in the device's sway; Steam's own fields?).
- [ ] Sound through speakers and headphones; volume keys (ClamShell reads them from evdev: which
      device sends them?); the volume slider (`wpctl`); the volume tab:
      apps listed while playing (Steam, Proton games), their levels and mutes, switching outputs.
- [ ] Brightness: one backlight per panel in `/sys/class/backlight`? Which name is which screen (for
      the brightness tab's labels)? Ranges, whether logind's
      SetBrightness works on them, and whether the lowest level (raw 1) is still visible.
- [ ] Home button: which device and key code it sends (BTN_MODE on the gamepad, or KEY_HOMEPAGE on a
      separate gpio-keys device?). ClamShell takes BTN_MODE from the grabbed pad only; Steam's overlay
      then never sees it.
- [ ] Steam with the virtual Xbox 360 pad: the real controller's USB ids, for a udev rule that clears its
      joystick tags (SDL and Steam skip it; ClamShell finds it by its capabilities) and closes its hidraw
      node (Steam reads some controllers through hidraw, past ClamShell's grab: every press twice). Steam
      then lists one controller. Steam's Xbox configuration support stays off (its default), so games read
      the virtual pad directly and ClamShell's keymaps are the only remapping; check that Steam's desktop
      layout doesn't act on the pad outside games (it would beside ClamShell's pad navigation).
- [ ] Lid: does the Thor report a lid switch (`SW_LID`), and does closing it suspend?
- [ ] Battery: capacity, charging, power draw in sysinfo; the charge limit setting (sysfs path?); UPower
      sees it (the battery mode's graph) and powers off at 3 %; GPU time per program in the battery mode
      (msm's `drm-engine-gpu` in fdinfo).
- [ ] Bypass charging (Android has it: plugged in, the device runs from the charger and the battery is left
      alone; less heat and wear while playing). What the battery's power supply offers (`ls
      /sys/class/power_supply/*/`): `charge_behaviour` (`inhibit-charge` is bypass, the standard way),
      `charge_control_end_threshold` (the Charge limit setting's: at or below the level now, charging
      stops and the system runs from the charger), or a vendor node (Qualcomm's downstream
      `charging_enabled` or the like). With the charger in and a game running, try each, watching the
      battery's `current_now` and `capacity`: bypass is the current near zero and the level holding. Then
      a Bypass choice on the Charge limit setting, by whichever works.
- [ ] Power profiles: cpufreq policies (3 clusters?), GPU devfreq, little-cores-only; the udev rule giving
      our user write access.
- [ ] Fan: is there a sysfs control?
- [ ] First boot: the root fills the card (`df -h /`), `/swapfile` is on (`swapon`), both command lines
      have `resume=` and `resume_offset=` (`cat /proc/cmdline` on the second boot, under each boot path),
      `journalctl -u thor-firstboot`. Time (timesyncd, `timedatectl`) and locale (`locale`).
- [ ] Screenshot: Print on a USB keyboard (`clamshell ctl screenshot`, grim).
- [ ] Suspend (ROCKNIX's deep suspend, ~3 s resume) and hibernate: `systemctl hibernate`, then resume
      (from the ABL path and the UEFI path); every subsystem after resume (sound, Wi-Fi, Bluetooth, GPU,
      both screens and touch, gamepad); lid closed for over 30 minutes ends in hibernate; UPower's
      hibernate at 3 %. After a resume: ClamShell logs "woke from sleep" and "settings applied", and both
      screens redraw. In the x86 VM the image is written and found, but the resumed machine
      sometimes resets at once, or QEMU's ACPI S4 wakes the hibernating kernel; one resume went through.
- [ ] The launcher under real use: tools/vm/vm.py's checks again, on the device.
- [ ] FEX for x86 games (32-bit Windows games are the open question).

## References

- ROCKNIX's Thor page (the Boot Mode note): <https://rocknix.org/devices/ayn/thor-sm8550/>
- ROCKNIX: <https://github.com/ROCKNIX/distribution> (SM8550 options: `BOOTLOADER="qcom-abl"`,
  `KERNEL_TARGET="Image"`), ABL: <https://github.com/ROCKNIX/abl>
- Thorch (Arch Linux ARM on ROCKNIX's Thor work, the closest model): <https://github.com/thorch-os/thorch>
- Pocknix (Arch Linux ARM for SM8250/SM8550 handhelds): <https://github.com/shuuri-labs/pocknix-os>
- Kettle Linux's Thor notes (stock U-Boot/UEFI, screens, touch, audio, power, gamepad):
  <https://github.com/kettlelinux/kettlelinux/blob/main/docs/THOR.md>
- Our tools: `tools/vm/vm.py` (virtual Thor, x86-64 and aarch64; `thor-image`), `tools/image/thor-image.sh`,
  `tools/image/bootimg.py`, `clamshell/tests/nested-sway.sh`
