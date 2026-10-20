# Game plan: when the AYN Thor arrives

The device: AYN Thor Max (16 GB / 1 TB, Snapdragon 8 Gen 2 / SM8550, 6" top + 3.92" bottom OLED).
The goal: our own minimal Arch Linux ARM with sway and ClamShell, booted from SD card first,
Android kept. Written 2026-10-14, before having the device; everything here is unverified on it.

## How the boot works

- **Stock: AYN's U-Boot with UEFI** (Kettle Linux's BOOT.md, BOOTLOADER.md and THOR.md, from the ABL
  disassembled and a Thor booted). The fastboot menu (hold Power and Volume down) has a default-boot
  switch, Android or **Loader**; Loader starts AYN's U-Boot (`loader_a`/`loader_b`, an Android boot image
  holding it), whose environment is `bootcmd=bootefi bootmgr` (AYNTechnologies/u-boot, branch
  `ayn-sm8550`, `board/qualcomm/ayn.env`): it boots `\EFI\BOOT\BOOTAA64.EFI`, from the SD card or internal
  storage (which first: not documented). No `/KERNEL`, no extlinux. U-Boot's own device tree is the Odin
  2's (its SMBIOS says "AYN Odin 2"), so the EFI program must load the Thor's: ours is systemd-boot with
  a `devicetree` line (Kettle uses GRUB). **The switch needs an unlocked bootloader**: locked, the menu
  says "Unlocked bootloader required", and unlocking erases Android's data. Which firmware has it isn't
  known (Kettle: "mentioned once, for firmware 355"; one report of a menu with Android/UEFI instead,
  as on the Odin 3). Verify on the device (day one, step 2) before flashing anything.
- **Else ROCKNIX's ABL** v1.2 (in 20261001: `rocknix_abl/` on the image's FAT partition, with
  `backup_abl.sh`, `flash_abl.sh`, `restore_backup_abl.sh`: plain `dd` of `abl_a` and `abl_b` to and from
  `/sdcard/rocknix_abl/`, run as root through AYN Android's "Run Script as Root", Settings → Handheld
  Settings → Advanced). It finds the boot partition on internal storage, SD or USB; per Kettle (citing
  ROCKNIX's developers, unverified) it boots `\EFI\BOOT\BOOTAA64.EFI` when there is one and `/KERNEL` only
  without: our image has both, so it would go the EFI way under either bootloader. Its menu (hold Volume
  down at power-on): Boot mode Linux, Boot source SD card, and **Device model: AYN Thor**, which it needs
  before it boots anything (`no match found for DTB!` otherwise). The way back: `restore_backup_abl.sh`,
  or `fastboot flash abl_a`/`abl_b` with the backups; an ABL broken past fastboot needs EDL, and there is
  no public EDL package for the Thor. **ROCKNIX's own updates and its install to internal storage
  re-flash both ABL slots** (`updateabl`), even over stock: boot ROCKNIX from SD only, never update or
  install it. Its `next` moved to OpenABL (OpenHandheldCollective/OpenABL v1.1, 2026-10-19; it also boots
  `BOOTAA64.EFI`, BLS, UKIs, extlinux and `/KERNEL`), so its next release will flash OpenABL instead.
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
| Kernel | ROCKNIX's prebuilt one (`KERNEL` of the release); or, with `THOR_KERNEL=~/.cache/thor-vm/kernel`, our build of the same kernel with binder (`tools/image/thor-kernel.sh`, see "A kernel with binder" below): its Image, device trees and modules replace ROCKNIX's in both boot ways and `/usr/lib/modules`, recorded in `/usr/local/share/thor/kernel` |
| UEFI boot, on the same FAT partition (an EFI system partition) | systemd-boot as `EFI/BOOT/BOOTAA64.EFI`, the kernel as `/Image` (decompressed), the Thor's device tree as `/dtbs/qcs8550-ayn-thor.dtb`, our initramfs, a loader entry with our command line; split out by `tools/image/bootimg.py split` |
| `/KERNEL` on FAT `ROCKNIX` (GPT name `system`, legacy-boot attribute, at 16 MiB) | ROCKNIX's kernel and device trees, unchanged (ours: the gzip'd Image and the 14 device trees, put into ROCKNIX's boot image by `bootimg.py repack --kernel`); our initramfs (mkinitcpio: base udev modconf block thor-root-wait resume filesystems fsck, no autodetect) and command line (`root=LABEL=THOR_ROOT rw` + ROCKNIX's SM8550 parameters; the first boot adds `resume=`, `resume_offset=`), put into both boot ways by `thor-boot` (`tools/device/thor-boot`, repacking with `tools/image/bootimg.py`), as the image is built and again on the device after updates |
| Kernel modules, firmware | ROCKNIX's `SYSTEM` (same release), incl. Qualcomm firmware: for this device only, not for sharing; modules ours with `THOR_KERNEL` |
| Root: ext4 `THOR_ROOT` | `pacstrap` from Arch Linux ARM's mirrors (unpinned): base, sudo, mkinitcpio, NetworkManager, OpenSSH, sway, mako, foot, Xwayland, Mesa + `vulkan-freedreno`, PipeWire, Noto, Python, vim, grim, growpart (`cloud-guest-utils`), gptfdisk; RetroArch with cores for NES, SNES, N64, Game Boy, GBA, DS, Mega Drive, PlayStation and Dreamcast and XMB's assets (its menu in ClamShell's colours: `clamshell/retroarch/menu-theme.py`, needing Pillow), mpv, ffmpeg; for maintenance on the device git, Rust, base-devel, pacman-contrib |
| System | time zone (`THOR_TIMEZONE`, else the build machine's `/etc/localtime`, else UTC), locale (`THOR_LOCALE`, default `en_US.UTF-8`), systemd-timesyncd, NetworkManager, sshd, Bluetooth |
| Session | user `thor` (groups wheel, video, input, audio; passwordless sudo, my ssh key), tty1 autologin into sway through uwsm (`wayland-wm@sway.service`; `graphical-session.target` with ClamShell, mako and udiskie as its user services; see process-control.md); the session's files from `tools/device/session.sh` (also run by maintenance): sway config (DSI-2 top, DSI-1 bottom, ROCKNIX's output lines; Print: `clamshell ctl screenshot`; `clamshell/sway/clamshell.conf` included), mako's and foot's configs, the Maintenance app's desktop file, the dock's first pins (Steam, foot, Maintenance), the starter `library.json` (`tools/device/library.json`: Games with a folder per system in `~/Games/<system>`, Films and TV in `~/Videos`) unless there is one; ClamShell built natively for aarch64 as a user service (control pipe `$XDG_RUNTIME_DIR/clamshell-control`, `CLAMSHELL_SESSION=device`: it puts the device to sleep when idle and reads the volume and brightness keys itself), uinput rule |
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
   Our own kernel build (`THOR_KERNEL`) has no built-in initramfs, so there it doesn't arise.
2. Early storage: settled. sdhci-msm is built into ROCKNIX's kernel (its messages are in the Image), as
   are ext4 and f2fs.
3. Audio: ROCKNIX's UCM profiles are copied over `alsa-ucm-conf`'s (a pacman hook puts them back after
   an update of that package); under UEFI the card is named `ayn-AYNOdin2` (ROCKNIX has a profile for that name too).
4. Rotation: the bottom panel gets fbcon's angle as well; unverified that it wants the same. ROCKNIX
   fixed a sideways bottom screen after 20261001 (PR #3507: "Sway doesn't apply a transform to an output
   that is powered off"): check that the bottom's transform holds when it is turned off and on. Touch: the
   image has ROCKNIX's calibration matrix for both panels and its hwdb entry marking the touchscreens as
   not keyboards (they report `BTN_TOUCH`, which made wlroots count a keyboard in the seat). The top panel
   runs at 118.6 Hz uncalibrated (ROCKNIX PR #3489, after 20261001).
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
11. Steam: not installed (neither Steam nor FEX is an Arch Linux ARM package), though the dock's first
    pins include `desktop:steam`. Installed by hand on the device (see "Steam" below), with a
    `steam.desktop` of its own so the pin and ClamShell's Steam entry find it.

## Before the device arrives

- [x] Build the image and check it (done 2026-10-14; see above).
- [x] Settle 2: sdhci-msm is built in (checked 2026-10-14, strings in the Image; f2fs too).
- [ ] Rebuild nearer the time: a newer ROCKNIX release, current packages.
- [x] A kernel with binder, for Android (Waydroid): ROCKNIX's SM8550 config has
      `# CONFIG_ANDROID_BINDER_IPC is not set` (checked 2026-10-19, its `next` branch and tag 20261001),
      and the option is built-in only. Done 2026-10-19: `tools/image/thor-kernel.sh` builds ROCKNIX's
      kernel from their git tag (default 20261001) on the PC with LLVM (3 minutes on 24 threads):
      Linux 7.2 from kernel.org, their 65 patches in their build's order (4 `mainline` and 6 `7.2` of
      the project, 55 of SM8550: panels, the Thor's RSInput gamepad and haptics, audio, GPU, fan),
      their device tree sources and the 14 SM8550 device trees of `config.xml`, their device config
      with their build's changes (CIFS and iSCSI off, wireless-regdb's `regulatory.db` built in). No
      out-of-tree modules (SM8550 has none). Ours: binder and binderfs on with no boot-time devices
      (`ANDROID_BINDER_DEVICES=""`; Waydroid makes them in binderfs), DMA-BUF heaps on
      (`/dev/dma_heap/system`, which Waydroid passes to Android's allocators), release `7.2.0-thor`, no
      built-in initramfs. Waydroid needs nothing more: no ashmem (without `/dev/ashmem` it sets
      `sys.use_memfd=true`), and PSI, namespaces, cgroups, veth, bridge and nftables NAT/masquerade
      (waydroid-net.sh's nft rules) are in ROCKNIX's config. Needs `bc` on the PC (the build computes the timer
      constants and the OIDs' encodings with it; built 2026-10-19 with a stand-in for those two, checked
      against their known values).
      Build it, then the image with it: `tools/image/thor-kernel.sh` (into `~/.cache/thor-vm/kernel`),
      `THOR_KERNEL=~/.cache/thor-vm/kernel tools/vm/vm.py --arch aarch64 thor-image`.
      Checked on the PC: the config matches ROCKNIX's embedded one except for binder, DMA-BUF heaps,
      hostname, initramfs, release and compiler-dependent options (clang turns on BTI and the 32-bit
      vDSO); the same 275 modules; the Thor's device tree byte-identical to the one in ROCKNIX's
      `/KERNEL`; `/KERNEL` repacked with our kernel by `bootimg.py repack --kernel`, and split back to the
      same Image and device tree. Not done: an SD image with it (thor-image.sh's `THOR_KERNEL` path not
      run end to end). Unverified until the device: that it boots at all (a clang build of a kernel
      ROCKNIX only builds with GCC), and the rest as for ROCKNIX's kernel; binderfs and Waydroid on it.
- [x] Find out what the stock Linux boot mode is. Done 2026-10-20 (see "How the boot works"): AYN's U-Boot
      through the fastboot menu's Loader switch, booting `BOOTAA64.EFI`; it needs an unlocked bootloader;
      which firmware has it is still unknown.
- [x] ROCKNIX's ABL steps and the way back, Thorch's and Pocknix's notes. Done 2026-10-20 (see "How the
      boot works"). Thorch and Pocknix don't flash an ABL: both expect ROCKNIX's already installed
      (Pocknix: v1.1.7 or newer; its Thor support not yet run on hardware).
- [ ] An SD card and a USB-C keyboard and Ethernet adapter. The slot runs at High Speed, not SDR104 (ROCKNIX's
      device tree masks SDR50/SDR104; Kettle measured about 13 MB/s), so a fast card buys little: any
      decent A1/A2 card. At that speed a hibernate image of a few GB takes minutes to write, and games
      load slowly from SD: worth a look at what the SDR104 node Kettle lists would take. USB-C is
      dual-role and the kernel has hub, HID and USB Ethernet drivers; a hub with both on a Thor isn't
      reported anywhere. Without a keyboard: ROCKNIX's USB network gadget (169.254.170.2) on the
      known-good image. No serial console is known.
- [x] A known-good image to compare against: ROCKNIX 20261001's SD image, downloaded and checked
      (`~/.cache/thor-vm/rocknix/ROCKNIX-SM8550.aarch64-20261001.img.gz`, sha256
      `2314561e75bc8dbbc91a7dd5aac719d49ed98d07428d8d0967797a79c0b3b16f`). Pocknix v0.4.0 for comparison if
      needed (10 GB: https://pocknix.shuuri.net/images/pocknix-sm8550-20260912-sd.img.zst, sha256
      `c67f3697511a46a04e7e70663b6055c45c00b688462ea0fce2dad3999ac9ba47`).

## Day one, in order

1. Boot Android, update its firmware, note the versions (Android build, bootloader). Charge.
2. **Check for a stock Linux boot mode before flashing anything**: enter the fastboot menu (hold Power and
   Volume down) and look for the default-boot switch (Android / Loader, or Android / UEFI). Note exactly
   what it offers, and whether the bootloader is locked (it says "Unlocked bootloader required"). If the
   switch works, skip step 4: no ROCKNIX ABL. If it needs unlocking, weigh that (it erases Android's data)
   against flashing ROCKNIX's ABL, which doesn't.
3. Back up first anyway: the stock ABL (ROCKNIX's `backup_abl.sh`) and anything else the install notes
   call out, copied off the device.
4. Only without a stock Linux mode: flash ROCKNIX's ABL (`flash_abl.sh`, as root in Android: "Run Script
   as Root"). Check that Android still boots. In its menu (hold Volume down at power-on): Boot mode Linux,
   Boot source SD card, Device model AYN Thor.
5. Boot a known-good image (ROCKNIX 20261001, downloaded) from SD: proves the bootloader, the SD slot and
   the screens. Don't update ROCKNIX or install it to internal storage (it re-flashes the ABL).
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
9. Steam, the native ARM64 client first (see "Steam").

## Steam

Steam's client is heavy (steamwebhelper is a Chromium), and on an ARM machine the question is whether the
client itself runs natively; games run through Proton or FEX either way, unless native ARM64.

1. First choice, Valve's native ARM64 client. Valve publishes it (since April 2026, its `linuxarm64`
   client manifest, stable and publicbeta) but links it nowhere and doesn't support it; the Steam Frame
   runs it. Its entry point is `steamrtarm64/steam` in the Steam folder. Needs: 4 KiB pages (`getconf
   PAGESIZE`; ROCKNIX's arm64 kernel should be 4 KiB) and `/lib/ld-linux-aarch64.so.1` (Arch Linux ARM
   has it). Games: Windows games through Valve's ARM64 Proton (Wine as ARM64EC, FEX inside), x86 Linux
   games through Valve's FEX compatibility tool, which the client downloads along with Steam Linux
   Runtime 4.0 for arm64 (not by default in early builds: check what it fetches). The host needs
   `binfmt_misc` and EROFS for FEX's root filesystem (`zcat /proc/config.gz`). Community packaging to
   copy from: steam-arm64-nix (fetches from the manifest, checks with `--doctor`). Reported rough edges:
   the client's own title bar doesn't move its window (no matter under sway), Big Picture's quick access
   menu froze once.
2. Else the x86 client under FEX, as Canonical's ARM64 snap does: FEX built from source (it left the AUR,
   and no longer supports Arch officially) with an x86-64 root filesystem. Then the whole client is
   emulated, and Valve's ARM64 Proton can't be used from it.

Then, either way, Steam made as light as it goes, measured before and after with `tools/inventory.sh`
(Steam's memory and CPU idle in its tray, and while a game runs):

- Started as ClamShell starts it, `-silent` (to its tray, no window).
- Its settings: Low Performance Mode (Library), shader pre-caching's background processing off unless a
  game stutters without it, no start with the computer (ClamShell starts it).
- Whether Steam idle behind a game costs much: if so, try Steam set to Pauses hidden in ClamShell (its
  downloads and friends stop while hidden; a game's need of the client while running decides it).

## Testing our system on the device

Each subsystem, then hibernate with each in use (hibernate matters more to me than suspend).

- [ ] Games paused when hidden (the default): a few native games and Proton games (FEX) frozen for
      minutes behind shelf, then shown: drawing, sound, the controller; any that jumps ahead or stalls
      after (its frame time step); whether a game hidden behind shelf still draws (then it is frozen
      after the grace) or stops (low CPU and GPU); whether loading and shader compiling are told from
      drawing (half a core, the GPU under 5 % on Freedreno); a frozen game's memory going to zram.
- [ ] An online game frozen behind shelf: whether it is dropped (then it is set to run).
- [ ] Sleep while busy: an idle session (sway, PipeWire, ClamShell, Steam in its tray, Waydroid up)
      stays under the busy thresholds (the user's processes 0.3 core, the network 100 kB/s), so it
      sleeps; a Steam download keeps it awake (`sleep: not now` in the journal).
- [ ] A second controller paired over Bluetooth while the Thor's own pad is grabbed: what ClamShell
      does with it (`CLAMSHELL_GAMEPAD`), and whether games get it as a second player.

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
- [ ] Drawing cost: how long a full redraw of each screen takes on the device's CPU (our own drawing, no
      GPU; on a 5900X 0.6 ms for the top screen, 0.2-0.35 ms for the bottom)
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
- [ ] Suspend and hibernate. Expect suspend to fail at first: ROCKNIX 20261001 turns SM8550's sleep off
      ("Sleep is currently broken": its `030-suspend_mode` quirk, power key ignored, a fake suspend
      instead), and its deep suspend is still an open PR (#3126, updated 2026-10-18: ~3 s resume, wake on
      the power key and the lid, 4-5 %/h on an Odin 2; with a PCIe fix "without which 7.2 never wakes up
      again", a 54 s freeze on the Thor put down to UFS idle, and the gamepad's fixes after resume). Our
      kernel is 20261001's, without it: if sleep fails, rebuild with #3126's patches. Kettle reports
      s2idle passing 5 `systemctl suspend` cycles on a Thor with its own kernel. Then hibernate: `systemctl hibernate`, then resume
      (from the ABL path and the UEFI path); every subsystem after resume (sound, Wi-Fi, Bluetooth, GPU,
      both screens and touch, gamepad); lid closed for over 30 minutes ends in hibernate; UPower's
      hibernate at 3 %. After a resume: ClamShell logs "woke from sleep" and "settings applied", and both
      screens redraw. In the x86 VM the image is written and found, but the resumed machine
      sometimes resets at once, or QEMU's ACPI S4 wakes the hibernating kernel; one resume went through.
- [ ] Time tab: the time zone set from the regions; "Set automatically" off and on (timesyncd); with it
      off, the date and time stepped (the bar's clock follows at once); 24-hour clock off and on.
- [ ] The session (only tried in the VM and nested on the desktop): autologin on tty1 starts sway through
      uwsm, `graphical-session.target` is reached, and ClamShell, mako and udiskie are active
      (`systemctl --user status`); launches run under `app-graphical.slice`. The failure path on the real
      console: with ClamShell made to fail (as in the VM: `ExecStart=/bin/false` in a drop-in), the
      session stops and tty1 says why: readable on the top screen (the right way up: the panel is
      portrait, fbcon may need `fbcon=rotate:`), and holding power then powering on starts it again.
      ClamShell isn't killed by its watchdog (30 s) while a heavy game loads.
- [ ] Steam's games, with the client that runs (see "Steam"; the follower and the found-window lineage
      were only seen on x86, a game itself is FEX or Proton's either way): a game started
      from ClamShell runs in its launch's scope (`systemd-cgls --user`), Steam's launcher for it found
      by its arguments (`pgrep -af SteamLaunch`: `SteamLaunch` and `AppId=<id>` as arguments of their
      own, as `steam_launchers` in `launcher.rs` expects, whatever FEX puts before them); the entry ends
      when the game does; a game started from Steam's own window is found by its window and followed the
      same way; Suspend and Quit act on the game, not Steam.
- [ ] ClamShell's idle cost (review 4.2): wakeups per thread, idle on shelf, as measured in the VM (see
      process-control.md, "ClamShell's own wakeups"), and the CPU of a redraw; then whether the clock's
      tick every second is worth limiting to when the clock can be seen.
- [ ] Archives browsed in the Files tab (ratarmount, a service per archive): mounted, opened, unmounted
      when left; ratarmount is in Arch Linux ARM's repositories (else the image build fails).
- [ ] The launcher under real use: tools/vm/vm.py's checks again, on the device.
- [ ] FEX for x86 games (32-bit Windows games are the open question).

## Ideas to try once the device is here

Held back until real use shows whether they are worth what they cost.

- The bar reserved on the bottom screen. The second screen is one layer surface, behind the windows moved
  there while they show, in front otherwise. Split into two, the bar (anchored to the top, always above
  windows) and the tab below it, the bar could claim its height as an exclusive zone, so windows moved
  down tile below it and the bar stays usable: its moved-down-windows button then acts like a tab (shows
  them; another tab brings ClamShell's view back), as Home+Select does. A toggle (Home+Y: still free),
  since it takes some 80 of the 1080 lines from what is below (a dual-screen emulator's second screen
  wants all of them). Costs: the slider that comes up over the tab while one slides on the bar would
  belong to the tab's surface, which then has to be in front while sliding; pointing's positions on the
  tab's surface offset by the bar's height. Worth it if, with windows below, reaching for the bar (or
  Home+Select) turns out to happen often.

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
