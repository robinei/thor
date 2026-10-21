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
- **Else OpenABL** (github.com/OpenHandheldCollective/OpenABL, v1.1 of 2026-10-19; where ROCKNIX's `next` and
  the community are moving; downloaded: `~/.cache/thor-vm/openabl/abl_signed-SM8550.elf`, sha256
  `cd97748bbd1c5dc994b8a73a05e88955297f4c88384e3ca4028d043fc1b16848`). Qualcomm's ABL with a menu and
  standard Linux boot: from internal storage, SD or USB it tries `Boot####` options, then Boot Loader
  Specification entries (`\loader\entries\*.conf`, ours: `thor.conf`, its `devicetree` line taking
  precedence) and UKIs, then extlinux, then `\EFI\BOOT\BOOTAA64.EFI`, then an Android boot image
  (`/KERNEL`): all of our image's ways. Device model from the `.dtb` files in `\dtbs` (ours has the
  Thor's); display rotation, boot source, a verbose log on screen, EDL, device info in its menu; fastboot
  and Android (with verified boot) as before; settings in UEFI variables. Flashed to `abl_a` and `abl_b`:
  `fastboot flash` (unlocked), or as ROCKNIX's scripts do from Android ("Run Script as Root", `dd`; ROCKNIX's
  `next` ships an `openabl/` folder with the same three scripts). Linux reaches its UEFI variables (bootctl,
  boot counting, BootNext) only with its kernel patch adding SM8550 to qcom_scm's QSEECOM allowlist
  (`Linux/0001-firmware-qcom-scm-allow-QSEECOM-on-OpenABL-s-platforms.patch`, tested on SM6115 only): ours
  to add once the Thor boots with it.
- **Or ROCKNIX's ABL** v1.2 (in 20261001: `rocknix_abl/` on the image's FAT partition, with
  `backup_abl.sh`, `flash_abl.sh`, `restore_backup_abl.sh`: plain `dd` of `abl_a` and `abl_b` to and from
  `/sdcard/rocknix_abl/`, run as root through AYN Android's "Run Script as Root", Settings → Handheld
  Settings → Advanced). It finds the boot partition on internal storage, SD or USB; per Kettle (citing
  ROCKNIX's developers, unverified) it boots `\EFI\BOOT\BOOTAA64.EFI` when there is one and `/KERNEL` only
  without: our image has both, so it would go the EFI way under either bootloader. Its menu (hold Volume
  down at power-on): Boot mode Linux, Boot source SD card, and **Device model: AYN Thor**, which it needs
  before it boots anything (`no match found for DTB!` otherwise). The way back: `restore_backup_abl.sh`,
  or `fastboot flash abl_a`/`abl_b` with the backups; an ABL broken past fastboot needs EDL, with AYN's
  factory package (day one, step 3). **ROCKNIX's own updates and its install to internal storage
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

- Screens: top DSI-2, 1080x1920 at 120 Hz (ICNA3520); bottom DSI-1, 1080x1240 at 60 Hz only (CH13726A;
  Armada's panel patch `0057`, our DTB). Both natively portrait: sway needs a transform on each.
- Touch: the device tree swaps and inverts axes; Kettle fixes it with `LIBINPUT_CALIBRATION_MATRIX` in a
  udev rule (`61-kettle-thor-touch.rules`), and maps each touchscreen to its screen.
- Audio: card `AYN-Thor` (`ayn-AYNOdin2` under EFI: U-Boot reports the Odin 2's SMBIOS); UCM profiles
  (their `kettle-ucm-ayn`) and firmware `ayn/thor/*` incl. `AYN-Thor-tplg.bin`. The routes (Thorch's
  `ucm2/AYN/Thor/HiFi.conf`): speakers on PCM 0, with no hardware volume (software only); headphones on
  PCM 1 (`Headphone Jack`, which mutes the speakers); the internal mic on PCM 2 (DMIC3 through SWR_MIC7);
  DisplayPort's sound on `DP0 Jack`.
- Gamepad: InputPlumber (their `40-kettle-thor.yaml`); the AYN key as quick access. The kernel's rsinput
  driver registers "AYN Odin2 Gamepad", bus 0x0003, ids 2020:3001, phys `rsinput-gamepad/input0`; it is a
  serial device (serdev), so there is no hidraw node (Armada's `qcs8550-ayn-common.dtsi`, our DTB).
  Since Armada's patch `1301` the driver doesn't clip samples, so values can pass the range it reports.
- Gyro: the IMU is on the ADSP's sensor island, not on the gamepad and not an IIO device: Kettle reads it
  with libssc through hexagonrpcd and makes a uinput device of it, "Kettle Motion Sensors"
  (`packages/kettle-motion/kettle-motiond.c`), axes turned by the matrix `0,-1,0,1,0,0,0,0,1` (its
  `devices.conf`, tried in a game 2026-10-07). Not in our image.
- Stick lights: `/sys/devices/platform/multi-led{l,r}{1-4}/leds/rgb:*/multi_intensity` and `power-led`
  (HTR3212); their channels are out of order (red is written `0 0 255`: ROCKNIX's `ledcontrol` and
  `battery_led_status`). Armada's kernel turns them off in suspend.
- Power: 4-18 W; at 5 W the three CPU clusters cap at 1344/1785/1977 MHz. The fan: Kettle reported a
  curve in the device tree, but in Armada's and ROCKNIX's device trees the thermal zones' trip points
  link to no cooling device (checked 2026-10-20 in our built DTB: only the GPU's throttling is linked), so
  the fan runs at the speed the kernel gives it at boot (70 of 255, ROCKNIX's patch 0500) until userspace
  sets it (Armada's armada-powerd, Thorch's thorch-fancontrol; ours: `clamshell-fan`). Its tachometer
  gives rpm in `fan1_input`; the device tree's `cooling-levels` stop at 150 of 255.
- Lid switch: sleep on close (`wakeup-event-action`); suspend (s2idle) verified.
- Backlights: top `ae96000.dsi.0`, bottom `ae94000.dsi.0` (Kettle's own kernel labels it `bottom-panel`).
- Battery: `power_supply` named `battery` (ROCKNIX's patch `0503`, in Armada's kernel; `qcom-battmgr-bat`
  without it); ClamShell and the udev rule find it by its type.
- USB-C display out: plugging in a DisplayPort monitor hard-freezes the device on Armada's 7.2.3 (Armada
  #529, open).
- The bottom screen draws about 0.7 W at idle. Wi-Fi and Bluetooth work.

## Our image (`tools/vm/vm.py --arch aarch64 thor-image`, built in the ARM virtual machine)

What goes in, and from where:

| Part | Source |
|---|---|
| Bootloader | not in the image: stock U-Boot (UEFI), else ROCKNIX's ABL flashed once (see below) |
| Kernel | Armada's, as `tools/image/thor-kernel.sh` builds it (`THOR_KERNEL=~/.cache/thor-vm/kernel`; Linux 7.2.6 with their 222 patches at commit 5643b1ab5156, 2026-10-20; binder in their config): its Image, SM8550 device trees and modules in both boot ways and `/usr/lib/modules`, recorded in `/usr/local/share/thor/kernel`. Without `THOR_KERNEL`, ROCKNIX's prebuilt one (`KERNEL` of the release) |
| UEFI boot, on the same FAT partition (an EFI system partition) | systemd-boot as `EFI/BOOT/BOOTAA64.EFI`, the kernel as `/Image` (decompressed), the Thor's device tree as `/dtbs/qcs8550-ayn-thor.dtb`, our initramfs, a loader entry with our command line; split out by `tools/image/bootimg.py split` |
| `/KERNEL` on FAT `ROCKNIX` (GPT name `system`, legacy-boot attribute, at 16 MiB) | ROCKNIX's kernel and device trees, unchanged (ours: the gzip'd Image and the 14 device trees, put into ROCKNIX's boot image by `bootimg.py repack --kernel`); our initramfs (mkinitcpio: base udev modconf block thor-root-wait resume filesystems fsck, no autodetect) and command line (`root=LABEL=THOR_ROOT rw` + ROCKNIX's SM8550 parameters; the first boot adds `resume=`, `resume_offset=`), put into both boot ways by `thor-boot` (`tools/device/thor-boot`, repacking with `tools/image/bootimg.py`), as the image is built and again on the device after updates |
| Kernel modules, firmware | ROCKNIX's `SYSTEM` (same release), incl. Qualcomm firmware: for this device only, not for sharing; modules ours with `THOR_KERNEL` |
| Root: ext4 `THOR_ROOT` | `pacstrap` from Arch Linux ARM's mirrors (unpinned): base, sudo, mkinitcpio, NetworkManager, OpenSSH, sway, mako, foot, Xwayland, Mesa + `vulkan-freedreno`, PipeWire, Noto, Python, vim, grim, growpart (`cloud-guest-utils`), gptfdisk; RetroArch with cores for NES, SNES, N64, Game Boy, GBA, DS, Mega Drive, PlayStation and Dreamcast and XMB's assets (its menu in ClamShell's colours: `clamshell/retroarch/menu-theme.py`, needing Pillow), mpv, ffmpeg; for maintenance on the device git, Rust, base-devel, pacman-contrib; zram-generator |
| System | time zone (`THOR_TIMEZONE`, else the build machine's `/etc/localtime`, else UTC), locale (`THOR_LOCALE`, default `en_US.UTF-8`), systemd-timesyncd, NetworkManager, sshd, Bluetooth |
| System files | from `tools/device/system.sh` (also run by maintenance): the fan daemon `clamshell-fan` (`clamshell/crates/fan`, a system service from early in boot: the fan's speed every 3 s from the mean of the three hottest CPU and GPU sensors, smoothed, on Armada's moderate curve with its ramp, by the mode ClamShell's Fan setting writes to `/run/clamshell-fan`; stopped or failed, `clamshell-fan --safe` leaves it at half speed); the `power` group's write access to what ClamShell's settings set (cpufreq caps, CPUs' online, the GPU's devfreq cap, the battery's charge limit and charge current, the power key's wakeup; udev rule `73-thor-power.rules`); the sticks' range (`/etc/modprobe.d/thor-rsinput.conf`); zram (`/etc/systemd/zram-generator.conf`: zstd, priority 100, before the swap file) and swapping tuned for it (`/etc/sysctl.d/70-thor-zram.conf`, Armada's values); the SD card's scheduler (BFQ, 4 MB readahead: `74-thor-sd.rules`); PipeWire's buffers (`70-thor-buffers.conf`, Armada's, against crackling after sleep); the checks at each boot (`thor-selftest.service`, `tools/device/thor-selftest`: see "Day one"); a log of each wake's cause (`/var/log/thor-wake.log`, a system-sleep hook); Steam's starter (`/usr/local/bin/steam`, `steam-setup`, `steam.desktop`) |
| Session | user `thor` (groups wheel, video, input, audio, power; passwordless sudo, my ssh key), tty1 autologin into sway through uwsm (`wayland-wm@sway.service`; `graphical-session.target` with ClamShell, mako and udiskie as its user services; see process-control.md); the session's files from `tools/device/session.sh` (also run by maintenance): sway config (DSI-2 top, DSI-1 bottom, ROCKNIX's output lines; Print: `clamshell ctl screenshot`; `clamshell/sway/clamshell.conf` included), mako's and foot's configs, the Maintenance app's desktop file, the dock's first pins (Steam, foot, Maintenance), the starter `library.json` (`tools/device/library.json`: Games with a folder per system in `~/Games/<system>`, Films and TV in `~/Videos`) unless there is one; ClamShell built natively for aarch64 as a user service (control pipe `$XDG_RUNTIME_DIR/clamshell-control`, `CLAMSHELL_SESSION=device`: it puts the device to sleep when idle and reads the volume and brightness keys itself), uinput rule |
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
   an update of that package); under UEFI the card is named `ayn-AYNOdin2`, whose profile in ROCKNIX's
   set is the Odin 2's (no internal mic, the speakers' volumes unset): the image gives that name the
   Thor's profile.
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
   it suspend-then-hibernate never gets to hibernate); the first boot's growpart, swap file and command
   line rewrite were run in the aarch64 VM on 2026-10-20 (a copy of the image, enlarged to 40 GB, booted
   with the VM's kernel): partition 2 grown to the card and its ext4 resized online, a 16 GiB /swapfile
   in fstab, and `resume=LABEL=THOR_ROOT resume_offset=403456` (the file's first block, checked with
   debugfs) in the loader entry and in /KERNEL (thor-boot, with the initramfs rebuilt for 7.2.6-thor).
   Not run: resuming from it, nor anything that needs the Thor's own hardware. If the card is not up when
   the initramfs's resume runs (thor-root-wait gives it 10 s) the hibernated state is lost and the
   machine boots fresh.
9. The fan: `clamshell-fan` takes it (`pwm1_enable` 3) and sets it every 3 s; tried only on a copy of
   /sys (its tests). The sensors' types starting with `cpu` or `gpu` and the chip's name `pwmfan` are
   checked in our DTB. Check on the device that the fan stops below 50 °C (Auto; ours alone, Armada's
   never stops) and starts again cleanly at 55 °C (whether it spins up from 51/255, Armada's minimum),
   speeds up under load (sysinfo shows its rpm), and that Quiet and Max change it;
   `systemctl stop clamshell-fan` leaves it at half speed. How loud Auto is at a game's usual
   temperature, and whether the hottest sensors' mean lags a GPU spike too long.
10. Test image only: passwordless sudo, sshd with a key, autologin without a password.
11. Steam: the client isn't in the image (it is Valve's, downloaded into the user's home); its first
    start (`/usr/local/bin/steam`, from the dock's pin or a Steam game) runs `steam-setup` in a terminal
    first (see "Steam" below). Its downloads and checksums were tried on 2026-10-20 (on the PC, into a
    scratch home); the client itself has never run here.

## Before the device arrives

- [x] Build the image and check it (done 2026-10-14; see above).
- [x] Settle 2: sdhci-msm is built in (checked 2026-10-14, strings in the Image; f2fs too).
- [ ] Rebuild nearer the time: a newer ROCKNIX release, current packages.
- [x] A kernel with binder, for Android (Waydroid). Since 2026-10-20 `thor-kernel.sh` builds Armada's kernel
      instead (binder is in their config); what follows is the ROCKNIX build it replaced. ROCKNIX's SM8550 config has
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
      (waydroid-net.sh's nft rules) are in ROCKNIX's config. (Armada's config, ours since, builds PSI but
      leaves it off, `CONFIG_PSI_DEFAULT_DISABLED`: our command line has `psi=1 cgroup_disable=pressure`,
      as Armada's.) Needs `bc` on the PC (the build computes the timer
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
- [ ] An SD card and a USB-C keyboard and Ethernet adapter. ROCKNIX's device tree masks SDR50/SDR104, so
      the slot runs at High Speed there (Kettle measured about 13 MB/s). Armada's, ours, lifts the mask
      but caps the clock at 37.5 MHz (`max-sd-hs-hz`), so a UHS card may negotiate SDR104 at that clock:
      untried on a Thor (Thorch reverted UHS as unstable; Armada #564 is a card failing, probably a
      Kingston). Any decent A1/A2 card; at these speeds a hibernate image of a few GB takes minutes to
      write, and games load slowly from SD. USB-C is
      dual-role and the kernel has hub, HID and USB Ethernet drivers; a hub with both on a Thor isn't
      reported anywhere. Without a keyboard: ROCKNIX's USB network gadget (169.254.170.2) on the
      known-good image. No serial console is known.
- [x] A known-good image to compare against: ROCKNIX 20261001's SD image, downloaded and checked
      (`~/.cache/thor-vm/rocknix/ROCKNIX-SM8550.aarch64-20261001.img.gz`, sha256
      `2314561e75bc8dbbc91a7dd5aac719d49ed98d07428d8d0967797a79c0b3b16f`). Pocknix v0.4.0 for comparison if
      needed (10 GB: https://pocknix.shuuri.net/images/pocknix-sm8550-20260912-sd.img.zst, sha256
      `c67f3697511a46a04e7e70663b6055c45c00b688462ea0fce2dad3999ac9ba47`).

### What Thorch and Armada do that we don't (read 2026-10-20)

Thorch (github.com/thorch-os/thorch) is stale: kernel 7.1.2 and ROCKNIX `next` of July; its move to 7.2
was closed unmerged. Armada (github.com/armada-os/armada) is active: its own Linux 7.2.6 with about 190
patches, each with its source in `packages/kernel/PATCHES.md`; Thor sleep power 0.77 → 0.46 W at the USB
input (commit 431bf57). Both run both panels at the same rotation, which settles that question.

For our image (small):
- [x] Pin s2idle: `MemorySleepMode=s2idle` in our sleep.conf.d (both projects ended up there; Thorch:
      `thorch-bsp/payload/etc/systemd/sleep.conf.d/50-thorch-native-suspend.conf`). Without it the kernel
      picks deep where firmware offers it.
- [x] `fbcon=rotate:1` on the command line, as Thorch requires: `thor-outputs` turns both outputs by
      fbcon's rotate, which is 0 if the kernel doesn't set it. Keep `/KERNEL`'s command line under 512
      bytes (Armada: the ABL may not read past that).
- [x] USB controller autosuspend (`power/control=auto` on the DWC3 device), so it doesn't hold a DDR vote
      while idle (Thorch `99-thorch-sm8550-usb-autosuspend.rules`, Armada `60-armada-usb-autosuspend.rules`).
- [x] Backlight names for the Brightness tab: top `ae96000.dsi.0`, bottom `ae94000.dsi.0` (not Kettle's
      "bottom-panel").

Our kernel is Armada's (decided 2026-10-20: the furthest community work, our own patches on top once we
have the device): `tools/image/thor-kernel.sh` builds their latest (`ARMADA_REF`, default `main`; the
commit is recorded in the build's `source`), with our config lines and `tools/image/kernel/series` (empty
for now). So these come with it: the gamepad across sleep (`0508`, `0515`, `1005`–`1007`, `0534`), rumble's
fixes (`1003`, `1004`), the s2idle stack (`0523`–`0526`, `0512`, `0522`, `0566`, `0204`, `0504` instead of
ROCKNIX's IPCC change), the wake sources (lid on opening only, volume up not), PCIe WAKE#, the codec's rails
in s2idle, and the charge current limit (`0903`: 0 runs the device from the charger, for bypass).
Candidates for our own patches, once tried on the device:
- [ ] OpenABL's QSEECOM allowlist patch, if we boot through OpenABL (UEFI variables from Linux).
- [ ] The bottom touchscreen kept powered in suspend (`edt,retain-power-in-suspend` on its node: Thorch's
      `dts-patches/0005`, Kettle's device tree; the FT5452's power-off path is unsafe), if it misbehaves
      after sleep; with Armada #757 (a contact kept after fingers lift; a candidate patch, open).
- [ ] The top panel's refresh calibration (it runs at 118.6 Hz): ROCKNIX's `next` after 20261001 has a
      kernel patch and a `040-panel_calibration` quirk for the Thor; Armada has neither.
- [ ] The thermal sensors' wake interrupts off (Thorch's `0003-thermal-qcom-tsens-skip-ayn-thor-uplow-wake-irq`,
      or Kettle's `60-kettle-tsens-nowake.rules` in userspace), if `/var/log/thor-wake.log` shows tsens
      waking it.
Done in userspace instead (2026-10-20), so no patch of ours:
- [x] The stick range: Thorch sets `axis-range = <1024>` on the gamepad node (its `dts-patches/0007`);
      Armada's device tree doesn't (they calibrate through rsinput's module parameters); without either,
      games see about 73 % of the sticks' travel (±1024 of the driver's ±1408). Ours: the same
      parameters, `axis_*_min=-1024 axis_*_max=1024`, in `/etc/modprobe.d/thor-rsinput.conf`
      (`tools/device/system.sh`). Check with evtest that the corners reach the axes' ends.
- [x] Armada's `system-sleep/50-armada-powerbutton-suspend` (no wake by the power key with the lid
      closed): ClamShell turns the power key's wakeup off as the lid closes and on as it opens
      (`clamshell-system::lid`). `40-armada-wake-ledger` (what woke it): our `thor-wake-log` hook,
      `/var/log/thor-wake.log`.

To know:
- Hibernate: nobody has tried it on the Thor (Thorch sets `AllowHibernation=no`, its sleep config saying
  its power patches are written for s2idle; Armada masks it). Our suspend-then-hibernate stays, but is
  tried on its own on day one; if hibernate doesn't come back, `SleepOperation=suspend` in our logind
  config.
- The AYN key is `KEY_F24` from its own device (`gpio-keys-ayn`), not the gamepad; ClamShell already reads
  it there (`pad::device::AynKey`). Back is `BTN_BACK` on the gamepad.
- Open on Armada: the bottom touchscreen keeping a contact after fingers lift (#757, a candidate patch,
  untested); charging at about 4 W against Android's 27 W (#89).
- SDR104: Thorch tried it and reverted ("unstable UHS modes"); Armada lifts the mask but keeps a 37.5 MHz
  clock cap. An experiment, not a fix.
- The full notes: Armada's per-patch sources, Thorch's sleep history; the HDR experiment can take
  Armada's ICNA3520 profile (colorimetry and 650 nits peak, its gamescope script `ayn.icna3520.oled.lua`).

## Day one, in order

1. Boot Android, update its firmware, note the versions (Android build, bootloader). Charge.
2. **Check for a stock Linux boot mode before flashing anything**: enter the fastboot menu (hold Power and
   Volume down) and look for the default-boot switch (Android / Loader, or Android / UEFI). Note exactly
   what it offers, and whether the bootloader is locked (it says "Unlocked bootloader required"). If the
   switch works, skip step 4: no ROCKNIX ABL. If it needs unlocking, weigh that (it erases Android's data)
   against flashing ROCKNIX's ABL, which doesn't.
3. Back up first anyway: `tools/device/android-backup.sh`, run with "Run Script as Root", copies the
   bootloaders (abl, loader, xbl) and the device's own partitions (persist, devinfo, the radios'
   calibration and the rest AYN's packages don't hold or would overwrite) to `/sdcard/thor-backup/` with
   checksums; copy that folder off the device.

   **The way back, whatever breaks: AYN's factory packages** (from the AYN Discord's #thor-general-faq,
   in `~/Downloads/thor/`): `Thor_20260112.7z` and `Thor_20260206.7z` (1.0.0.377) are full flash packages
   for Qualcomm's emergency download mode (EDL): every partition's image (the stock `abl.elf`, AYN's
   U-Boot `loader.img`, boot, super, the firmware), `rawprogram*.xml` and `patch*.xml` for UFS, and the
   device programmer `xbl_s_devprg_ns.melf`. EDL is in the boot ROM, so this works with the ABL or anything
   after it broken. AYN's tutorial (the `.docx` beside them): QFIL (QPST 2.7.496, in the same folder;
   Windows), storage UFS, the `xbl` programmer, all the XMLs; the Thor off and connected by USB, hold Power
   and both volume keys until the computer sees the port (Qualcomm's 9008), then Download. On Linux,
   Linaro's `qdl` does the same: `qdl --storage ufs xbl_s_devprg_ns.melf rawprogram*.xml patch*.xml`. The
   full flash erases Android's data; leave `persist.img` out (edit it out of the rawprogram XML) unless
   persist itself is lost. For only the ABL, `fastboot flash abl_a`/`abl_b` with the stock `abl.elf` (or
   the backup) while fastboot still works. `Thor_20251120_ota.zip` is an ordinary Android OTA
   (`payload.bin`): rollback from Android's Local Update (renamed `update.zip`, 35 % battery or more).
4. Only without a stock Linux mode: flash OpenABL (downloaded; to `abl_a` and `abl_b`, by `dd` as root in
   Android, "Run Script as Root", after step 3's backup), ROCKNIX's ABL the fallback (`flash_abl.sh`).
   Check that Android still boots. In the menu (hold Volume down at power-on): Boot mode Linux, Boot source
   SD card, Device model AYN Thor (OpenABL: from `\dtbs`, or the entry's own).
5. Boot a known-good image (ROCKNIX 20261001, downloaded) from SD: proves the bootloader, the SD slot and
   the screens. Don't update ROCKNIX or install it to internal storage (it re-flashes the ABL).
   Note what ROCKNIX's session shows for outputs (`swaymsg -t get_outputs`), inputs
   (`swaymsg -t get_inputs`, `evtest`), the gamepad's device and buttons, sound devices, Wi-Fi.
6. Boot our image from SD. If it doesn't come up, the known issues above are the suspects, in order.
   Serial console: none known; a USB keyboard and the screen, or ssh over USB Ethernet. Each boot runs
   `thor-selftest` (from early on, not waited for): a pass at once (the kernel, its command line), and
   one 90 s later (firmware and remote processors, both screens and backlights, the input devices by
   name, the fan, the battery and the power group's access, zram and the swap file, the SD card's mode
   and scheduler, sound, Wi-Fi, Bluetooth, the session), each line `ok`, `FAIL` or `info`, into the
   journal (persistent, on the card) and `/var/log/thor-selftest.log` (the last 20 boots). If the boot
   gets that far but nothing shows, read them on the PC from the card's root:
   `journalctl --directory=MNT/var/log/journal -t thor-selftest` (and `-b -1` etc. for earlier boots;
   the whole journal is there too), or the log file. By hand on the device: `sudo thor-selftest --now`.
   A boot that fails before the root is mounted leaves nothing on the card.
7. Library: put games in `~/Games/<system>` and videos in `~/Videos/Films` and `~/Videos/TV` (a folder per
   series); add a TMDB key to `~/.config/clamshell/library.json` (`"keys": { "tmdb": "..." }`) for films' and
   series' posters and overviews; Tools → Rescan library. PlayStation games need a BIOS for the
   PlayStation core (Beetle PSX HW: `scph5501.bin` or another it lists, in `~/.config/retroarch/system`;
   the old PCSX ReARMed, which didn't, is a 2016 package in Arch Linux ARM that no longer installs beside
   `libretro-core-info`).
8. Maintenance: install Claude Code (`curl -fsSL https://claude.ai/install.sh | bash`) and log in; make
   the device's ssh key (`ssh-keygen -t ed25519`) and add it to the GitHub repository (a deploy key with
   write access); then a first `/maintain` from the dock's Maintenance app.
9. Steam, the native ARM64 client first (see "Steam").

## Steam

Steam's client is heavy (steamwebhelper is a Chromium), and on an ARM machine the question is whether the
client itself runs natively; games run through Proton or FEX either way, unless native ARM64.

Valve's native ARM64 client (researched 2026-10-20). Valve publishes it on its update servers, stable
(`steam_client_linuxarm64`) and beta (`steam_client_publicbeta_linuxarm64`), but links it nowhere and
closes ARM bug reports as unsupported. Armada (beta, `steamdeck_publicbeta`), Thorch (beta) and ROCKNIX
(since 2026-04) all run it on the Thor. It is linked for 4 KiB pages (the Thor's kernel has them). Windows
games run through Valve's "Proton 11.0 (ARM64)" (Wine as ARM64EC, FEX inside), which Steam fetches like
any Proton: nothing on the system. x86 Linux games would need FEX with an x86 root filesystem on the
system (Valve's FEX compatibility tool uses it; Armada packages it, Thorch takes ROCKNIX's build); no
Arch Linux ARM package, so not here for now. The x86 client under FEX (Canonical's snap) is the
fallback, slower (its UI emulated).

Ours: `tools/device/steam-setup`, after Thorch's installer for Arch Linux ARM, run as the user on the
first start of `/usr/local/bin/steam` (`tools/device/steam`, in a terminal):
- Valve's runtime for the client (steamrt3c arm64, `repo.steampowered.com/steamrt3c/images/<version>/`,
  checked against its SHA256SUMS) and the client's binaries (the beta manifest's
  `bins_linuxarm64_linuxarm64` zip, checked against the manifest's sha2) into `~/.local/share/Steam`.
  The zip names a few links and a folder with backslashes (moved where they belong: `steamrtarm64/libs/`'s
  `libnghttp2.so.14` and others) and makes its files world-writable (taken away).
- Links: `~/.steam/{steam,root,sdkarm64,steamrtarm64}`, and in `~/.local/share/Steam/lib/aarch64-linux-gnu`
  the older libraries the client wants that Arch Linux ARM has newer or none of (gtk2's and ibus's from
  Valve's runtime, libvpx 6 from the client); `steamrtarm64/steam-launch-wrapper` to
  `../linuxarm64/steam-launch-wrapper` (Steam's own update removes it, and then every Proton launch exits
  with 127: Thorch's `thorch-repair-steam-arm64`); `~/.steam/sdk64` and `sdk32`'s `steamclient.so` from
  the first place that has one (Thorch's). Remade at each start and after an update. What it still lacks, `ldd` of
  `steamui.so` lists at the end.
- A first run (`steam -exitsteam`) that fetches the rest of the client. Then `steam` starts it as Armada's
  launch-steam does (its folder and the links first in the library path, the runtime's tools on the path,
  SYSTEM_PATH and SYSTEM_LD_LIBRARY_PATH kept for compatibility tools) with Thorch's flags (no
  verification, repair or update of the client against a bootstrap it didn't install), so
  `steam-setup --update` updates it (maintenance).
- Desktop mode, as ClamShell starts it (`-silent`, or `-silent -applaunch ID` for a game); no gamescope,
  no SteamOS stubs (those are for Big Picture's SteamOS mode).

Tried 2026-10-20 on the PC into a scratch home (the client's first run left out): the downloads, checksums,
backslash names and links. On the device: whether the first run and the start work as written; whether
`steam -silent` stays out of sight under sway; the beta's known issues (the quick access menu freezing
Big Picture on Adreno, Steam #13544; Remote Play's decoding on SM8550's video decoder, #13428).

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
- [ ] Gyro: the IMU is the ADSP's (see "The hardware"): neither evdev from the gamepad nor IIO. ClamShell
      reads an evdev motion sensor (INPUT_PROP_ACCELEROMETER, ABS_RX/RY/RZ, resolution in units per °/s)
      paired with the controller; for the Thor that takes hexagonrpcd and libssc in the image and either
      Kettle's kettle-motiond (a uinput device ClamShell could pair) or a source of our own behind the same
      seam (`pad/motion.rs`, `MotionSource`). First: Kettle's daemon run by hand, its axes, signs and units. Its offset at rest (°/s, and how it drifts warm): whether
      the still detection (under 5 °/s, within 0.5 °/s, for 1.5 s) catches it and doesn't eat slow aim,
      and "Calibrate gyro". Then the feel: gyro to the pointer at each speed (pixels per degree),
      acceleration and tightening's defaults, which way turning and tilting move it (`ROLL_TURNS` in
      `pad/remap.rs` for tilting), gyro to a stick with its minimum, holding a trigger to aim, and the
      ratchet.
- [ ] Rumble: a game's rumble on the controller's motors at each Rumble setting; the keymap's buzzes (a
      layer armed or toggled on, and off: `Buzz` in `pad/rumble.rs`, 40 ms at 40000 on the light motor,
      30000 on the heavy one) felt, and not confused with a game's.
- [ ] Drawing cost: how long a full redraw of each screen takes on the device's CPU (our own drawing, no
      GPU; on a 5900X 0.6 ms for the top screen, 0.2-0.35 ms for the bottom; the image and mask blends
      vectorise for NEON, checked in the aarch64 build: `ld4`/`st4` per channel, `umull`/`umlal`, `ushr`)
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
      If the pass costs enough (a few hundred mW at 120 Hz, not tens), the way without plane rotation:
      games render portrait themselves. wlroots' scene scans out a buffer whose buffer transform matches
      the output's, and tells clients the transform it prefers (`wl_surface.preferred_buffer_transform`;
      both to confirm in its source), so sway likely needs nothing. Mesa's Wayland WSI needs a patch:
      report the preferred transform as `currentTransform`, and set `wl_surface.set_buffer_transform`
      when an app presents with that pre-transform (as Android's WSI does). DXVK needs one too, a cheap
      one: its present pass already copies the back buffer into the swapchain image, so that copy writes
      it rotated into a 1080x1920 swapchain (VKD3D-Proton likewise, probably). Only through Wine's
      Wayland driver (`PROTON_ENABLE_WAYLAND=1`): an X11 window under Xwayland can't set a buffer
      transform. Native games gain nothing. Both patches are upstream candidates.
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
- [ ] Steam with the virtual Xbox 360 pad: the real controller (2020:3001, "AYN Odin2 Gamepad"; serdev,
      so no hidraw node to close) needs a udev rule that clears its joystick tags (SDL and Steam skip it;
      ClamShell finds it by its capabilities). Check that Steam lists one controller. Steam's Xbox configuration support stays off (its default), so games read
      the virtual pad directly and ClamShell's keymaps are the only remapping; check that Steam's desktop
      layout doesn't act on the pad outside games (it would beside ClamShell's pad navigation).
- [ ] Lid: does the Thor report a lid switch (`SW_LID`), and does closing it suspend? With the lid
      closed, the power key's wakeup is off (`cat /sys/class/input/input*/device/power/wakeup` of the
      `pm8941_pwrkey` device; ClamShell's log says "lid closed"), and a press doesn't wake it; opening the
      lid does. `/var/log/thor-wake.log` names what woke it each time. A race: logind sleeps on the same
      lid event, with no inhibitor holding it for ClamShell's write; close the lid a few times in a row
      and press the power key each time. If it ever wakes, a delay inhibitor (or a system-sleep hook, as
      Armada's) is the fix.
- [ ] Battery: capacity, charging, power draw in sysinfo; the charge limit setting
      (`charge_control_end_threshold` of `battery`, written by the power group). Kettle found the
      threshold has no effect on SM8550's firmware (its patch `1190`); with Armada's patch `0904` a write
      the firmware refuses fails, and ClamShell's log says so (once). If it does nothing: drop the 90 and
      80 choices (Bypass stays, if it works). UPower
      sees it (the battery mode's graph) and powers off at 3 %; GPU time per program in the battery mode
      (msm's `drm-engine-gpu` in fdinfo).
- [ ] Bypass charging (Android has it: plugged in, the device runs from the charger and the battery is left
      alone; less heat and wear while playing). Charge limit's Bypass writes 0 to the battery's
      `constant_charge_current` (Armada's patch `0903`: the battery firmware's charge current limit; the
      other choices write `constant_charge_current_max` back). With the charger in and a game running,
      watch the battery's `current_now` and `capacity`: bypass is the current near zero and the level
      holding. Then: whether it holds after unplugging and plugging in again (ClamShell writes it at each
      change of the settings, at its start, on waking, and every 30 s while Bypass is on), and after a
      reboot. Armada PR #677 warns that charge currents under 100 mA charge unstably on the Thor (500 mA
      the spec's minimum): watch for the charger dropping out or the battery draining with 0; if so,
      Bypass may need a small current instead, or to go.
- [ ] Power profiles: cpufreq policies (3 clusters?), GPU devfreq, little-cores-only; the power group's
      write access (`ls -l` of a policy's `scaling_max_freq`: group `power`, writable).
- [ ] Fan: `clamshell-fan` running (`systemctl status clamshell-fan`, its log names the fan and the mode);
      see known issue 9.
- [ ] First boot: the root fills the card (`df -h /`), `/swapfile` is on (`swapon`), both command lines
      have `resume=` and `resume_offset=` (`cat /proc/cmdline` on the second boot, under each boot path),
      `journalctl -u thor-firstboot`. Time (timesyncd, `timedatectl`) and locale (`locale`).
- [ ] Screenshot: Print on a USB keyboard (`clamshell ctl screenshot`, grim).
- [ ] Suspend and hibernate. Expect suspend to fail at first: ROCKNIX 20261001 turns SM8550's sleep off
      ("Sleep is currently broken": its `030-suspend_mode` quirk, power key ignored, a fake suspend
      instead), and its deep suspend is still an open PR (#3126, updated 2026-10-18: ~3 s resume, wake on
      the power key and the lid, 4-5 %/h on an Odin 2; with a PCIe fix "without which 7.2 never wakes up
      again", a 54 s freeze on the Thor put down to UFS idle, and the gamepad's fixes after resume). Our
      kernel is Armada's, which carries that work and more (s2idle, 0.77 → 0.46 W asleep on a Thor at the
      USB input): expect s2idle to work; its sleep power still far above a phone's. Kettle reports
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

From the review of 2026-10-21 (what Armada, ROCKNIX, Thorch and Kettle do, taken over untried here):

- [ ] Screens turned on the first session: `~/.config/sway/outputs` written (thor-session runs
      thor-outputs by its path; it wasn't on the login shell's PATH before).
- [ ] zram: `swapon --show` lists `/dev/zram0` (priority 100) and `/swapfile`; `sysctl vm.swappiness`
      is 180; a game frozen behind shelf goes to zram, and hibernate still writes to the swap file.
- [ ] The command line's additions: `/proc/pressure/` exists (`psi=1`; Waydroid's memory killer), and
      `/sys/module/ufshcd_core/parameters/uic_cmd_timeout` is 3000; no UFS errors after sleep (the 54 s
      freeze ROCKNIX saw).
- [ ] Sound under each boot path: the speakers, the headphones (plugging them in mutes the speakers)
      and the internal mic (`arecord`) both under the ABL path (`AYN-Thor`) and under U-Boot
      (`ayn-AYNOdin2`, now given the Thor's profile); no crackling after waking (PipeWire's buffers).
- [ ] The SD card: the mode it runs at (`/sys/kernel/debug/mmc0/ios`: SDR104 at 37.5 MHz, or High
      Speed) and its speed; card errors in the journal under load and after sleep (if any, put
      `sdhci-caps-mask` back as ROCKNIX has it). `cat /sys/block/mmcblk0/queue/scheduler` says bfq.
- [ ] The sticks: with the range at ±1024, the corners reach the axes' ends in `evtest`, and nothing
      goes past them (since Armada's `1301` the driver doesn't clip); if values pass ±1024, clamp them in
      the pad thread or use ROCKNIX's per-unit calibration (`gamepadcalibration`).
- [ ] The stick and power lights: lit at boot or not, off while asleep (Armada's kernel turns them
      off; ROCKNIX also does it from userspace before sleep, saying lit ones keep it awake).
- [ ] Steam: a Proton game starts (no exit 127: `steam-launch-wrapper`'s link), after Steam has
      updated itself too.
- [ ] GPU hangs (the GMU): if any, ROCKNIX's and Thorch's workaround, `cpu0/cpuidle/state1/disable`
      (Pocknix turns it back on around sleep).
- [ ] The bottom touchscreen after sleep: still working, no contact left behind (#757); else the
      `edt,retain-power-in-suspend` patch (see "Candidates for our own patches").
- [ ] USB-C display out: last, with nothing unsaved (Armada #529: a DisplayPort monitor hard-freezes
      the device).
- [ ] The gamepad back from sleep or a replug while a game runs: the game keeps its pad (the virtual pad
      no longer made again for a stick a little off centre), and rumble requests while it is gone don't
      stall the game.

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
