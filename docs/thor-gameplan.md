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
- Touch: ROCKNIX's and Kettle's device trees swap and invert the touchscreens' axes, and their
  `LIBINPUT_CALIBRATION_MATRIX` (`0 -1 1 1 0 0`, Kettle's `61-kettle-thor-touch.rules`) undoes it. Armada's
  device tree (ours) drops those properties, so the touchscreens report their panels' own portrait
  coordinates and need no matrix: wlroots turns touch with the output it is mapped to. Adding the matrix
  to our device tree would turn touch 90° wrong.
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

## Our image and packages

The system is Arch Linux ARM plus the Thor's own packages (`packages/`), so that a device stays current
with `pacman -Syu` alone. The packages are built on the PC for aarch64 (`tools/repo/build.sh`: makepkg
with `CARCH=aarch64`; ClamShell cross-compiled with rustup's aarch64 target, clang and lld against a
sysroot of Arch Linux ARM's packages, `tools/repo/sysroot.sh`), signed with the
repository's key and indexed (`tools/repo/repo.sh index`, into `~/.cache/thor-vm/repo`), published as the GitHub release `packages`
(`tools/repo/repo.sh publish`, with `gh`), and read by the devices' pacman as `[thor]`
(`Server = https://github.com/robinei/thor/releases/download/packages`, before `[core]`). The image
(`tools/image/image.sh`, on the PC too: this PC's pacman installing Arch Linux ARM's aarch64 packages,
their install scripts and hooks run through qemu-user's binfmt; `tools/image/thor-image.sh` as root) is
those packages pacstrapped onto an SD card, plus the little that is per device; thor-firmware is built
with the others but kept out of the published repository. Nothing in either is anyone's in particular.

| Package | What |
|---|---|
| `thor` | the metapackage that is the whole system: the packages below, Turnip, the day-one checks' tools, what maintenance needs; an image is `base` and this (and thor-firmware) |
| `linux-thor` | Armada's kernel, built in its PKGBUILD as Arch Linux ARM's linux-aarch64 is (cross-compiled with LLVM on the PC): kernel.org's tarball (its signature checked against Linus Torvalds' and Greg Kroah-Hartman's keys, `keys/pgp`) and Armada's repository at a pinned commit (`_armada`; Linux 7.2.6 with their 223 patches at d2bcf6fd3343, 2026-10-21, which reports the charge threshold's failures (0904); binder in their config), their series applied as their build does, our patches and config lines after; a release of its own per package version (`7.2.6-thor-d2bcf6f-3`: Armada's commit and pkgrel); modules, `Image` and the SM8550 device trees in `/usr/lib/modules/<release>`. Following Armada: `tools/repo/follow-armada.sh`. Its source package is published beside it (GPL-2.0) |
| `thor-system` | the boot: `thor-boot` puts the kernel (`/Image`, decompressed, an EFI program), the Thor's device tree, our initramfs (`/usr/lib/thor/mkinitcpio.conf`: base udev modconf block thor-root-wait resume filesystems fsck, no autodetect) and the loader entry's command line on the card's FAT partition for systemd-boot (`EFI/BOOT/BOOTAA64.EFI`), and the same into `/KERNEL` for the ABL (`bootimg.py repack`: the gzip'd Image with the 14 device trees, ROCKNIX's header); a pacman hook runs it after updates of the kernel, systemd, mkinitcpio, kmod, util-linux, zstd. The first boot (`thor-firstboot.service`, once): grows the root partition and its ext4 to the card; makes `/swapfile`, 16 GiB (only if 8 GiB stay free beside it), in fstab; puts `resume=LABEL=THOR_ROOT resume_offset=<first block>` on the command line. The power group's write access to what ClamShell's settings set (cpufreq caps, CPUs' online, the GPU's devfreq cap, the battery's charge limit and charge current, the power key's wakeup; `73-thor-power.rules`); the sticks' range (`/usr/lib/modprobe.d/thor-rsinput.conf`); zram (zstd, priority 100, before the swap file) and swapping tuned for it (Armada's values); the SD card's scheduler (BFQ, 4 MB readahead); PipeWire's buffers (Armada's, against crackling after sleep); sleep (logind: power button and lid do `sleep` = suspend-then-hibernate, else suspend; long press powers off; s2idle; `HibernateDelaySec=30min`); a log of each wake's cause (`/var/log/thor-wake.log`); the checks at each boot (`thor-selftest`: see "Day one"); the account helper `thor-account` (pkexec, polkit: the password and sshd, for ClamShell's Setup tab); the touchscreens' hwdb entry, uinput for the input group, USB controller autosuspend; the thermal sensors' wakeup off; the fan's, battery's and gamepad's drivers loaded at boot; Wi-Fi scanning with the device's own address (NetworkManager); the lid heeded 5 s after boot or waking |
| `thor-session` | tty1's autologin as `thor` into sway through uwsm (`thor-session`, from `/etc/profile.d/thor-session.sh`; `graphical-session.target` with ClamShell, mako and udiskie as its user services, enabled globally; see process-control.md); sway's config (`/usr/share/thor/sway`: DSI-2 top, DSI-1 bottom; both outputs turned 90° and the touchscreens' outputs written at each start into the runtime directory, `thor-outputs`; ClamShell's `clamshell.conf`; the user's additions in `~/.config/sway/config.d/`), foot's (through `XDG_CONFIG_DIRS`) and mako's configs (the user's own first); the home's starting files where missing, at each session start (`thor-home`: the starter `library.json` with its folders, the dock's first pins, mpv's and RetroArch's configs and RetroArch's menu theme); Steam's starter (`steam`, `steam-setup`, `steam.desktop`); the Maintenance app (`thor-maintain`: Claude Code with the `/maintain` playbook in `~/.local/share/thor/maintenance`) |
| `clamshell` | ClamShell and the fan daemon `clamshell-fan` (a system service from early in boot: the fan's speed every 3 s from the mean of the three hottest CPU and GPU sensors, smoothed, on Armada's moderate curve with its ramp, by the mode ClamShell's Fan setting writes to `/run/clamshell-fan`; stopped or failed, `--safe` leaves it at half speed); the power group (sysusers); `clamshell-android` |
| `thor-ucm` | the Thor's ALSA UCM profiles, ROCKNIX's patch (`0005_Add-AYN-Thor`, pinned to their commit c445081a), new files beside `alsa-ucm-conf`'s; the same profile under the Odin 2's names the card gets under AYN's U-Boot |
| `thor-keyring` | the repository's signing key (`pacman-key --populate thor`); its private half on the publishing PC only (`~/.config/thor-repo/gnupg`) |
| `thor-firmware` | the Thor's firmware from ROCKNIX's SM8550 release (20261001: the GPU's zap shader, ADSP/CDSP, the audio topology, Wi-Fi and Bluetooth, the video decoder's): AYN's and Qualcomm's, for this device only, so never published: built with the others, installed by the image build from `~/.cache/thor-vm/repo/local`, rebuilt on the device by maintenance after a ROCKNIX release |

The repository's key: ed25519, made for this alone (fingerprint 6C4C5B7FB11D609B739FDAC30BB252FF8CA9B43F).
Its private half is only on the publishing PC (`~/.config/thor-repo/gnupg`, without a passphrase so that
publishing asks nothing): back that folder up, offline. If it is lost: a new key, a new thor-keyring
carrying it, published; devices can't trust that update by themselves (it is signed by a key they don't
know), so each one takes it by hand once (`sudo pacman-key --recv-keys`/`--lsign-key` of the new
fingerprint, or `pacman -U` of the new thor-keyring from the release), and images built afterwards carry
it. If it leaks: the same, and the old fingerprint in thor-keyring's `thor-revoked`, so that devices which
update stop trusting it.

What the image adds:

| Part | Source |
|---|---|
| Bootloader | not in the image: stock U-Boot (UEFI), else ROCKNIX's ABL or OpenABL flashed once (see below) |
| FAT `ROCKNIX` (GPT name `system`, legacy-boot attribute, at 16 MiB, 512 MiB; an EFI system partition) | the loader's config and entry with our command line (`root=LABEL=THOR_ROOT rw` + ROCKNIX's SM8550 parameters, `fbcon=rotate:1`, `psi=1 cgroup_disable=pressure`, `ufshcd_core.uic_cmd_timeout=3000`, `panic=30`), ROCKNIX's `/KERNEL` as the header thor-boot repacks; thor-boot then fills it |
| Root: ext4 `THOR_ROOT` | `pacstrap` from Arch Linux ARM's mirrors (unpinned) and the local `[thor]`: base, sudo, NetworkManager, OpenSSH, Xwayland, Mesa + `vulkan-freedreno`, PipeWire, Noto, vim, grim, gptfdisk; RetroArch with cores for NES, SNES, N64, Game Boy, GBA, DS, Mega Drive, PlayStation and Dreamcast and XMB's assets, mpv, ffmpeg; evtest, alsa-utils; for maintenance pacman-contrib, base-devel, squashfs-tools, git; the Thor's packages (their dependencies bring sway, uwsm, mako, foot, udiskie, zram-generator) |
| System | hostname `thor`, time zone UTC (`THOR_TIMEZONE` for another), locale (`THOR_LOCALE`, default `en_US.UTF-8`), systemd-timesyncd, NetworkManager, Bluetooth; sshd installed but off; UPower: hibernate at 3 % (power off if it can't); `[thor]` in pacman.conf |
| User | `thor` (groups wheel, video, input, audio, power), its account locked until a password is set in ClamShell's Setup tab; sudo for wheel with that password (one timestamp for all the user's processes only during a maintenance pass: thor-maintain); the session's packages name it |

The image: `~/.cache/thor-vm/thor-sd.img`, 12 GB. Its first build (2026-10-14, ROCKNIX 20261001,
kernel 7.2.0, before the packages: 2.5 GB used). Checked from the host and in the build machine: GPT with partition 1 an EFI system
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
  off unless something shows there). Our session writes the same (`thor-outputs`), but turns both panels
  90° without reading fbcon (Armada's kernel config had no fbcon rotation until ours added it, so the
  file said 0; Kettle, Thorch and Armada all turn both panels by 90°).

Known issues and risks, to check first:

1. Our ramdisk is meant to be unpacked over the kernel's built-in initramfs, so our `/init` wins. If not,
   ROCKNIX's init runs, looks for its own `SYSTEM`, and the boot fails (ABL path only; the UEFI path
   passes our initramfs as the only one... unless the built-in one still unpacks first: same question).
   Our own kernel build (linux-thor) has no built-in initramfs, so there it doesn't arise.
2. Early storage: settled. sdhci-msm is built into ROCKNIX's kernel (its messages are in the Image), as
   are ext4 and f2fs.
3. Audio: ROCKNIX's Thor profiles (thor-ucm) are new files beside `alsa-ucm-conf`'s, which they
   include (its WCD938x and LPASS sequences), so an update of alsa-ucm-conf that changes those can change
   the Thor's sound: the self-test lists the card, sound itself is a day-one check. Under UEFI the card is
   named `ayn-AYNOdin2`; thor-ucm gives that name the Thor's profile (ROCKNIX's is the Odin 2's: no
   internal mic, the speakers' volumes unset).
4. Rotation: both panels turned 90° (sway's `transform 90`, the panels' "right side up" orientation in
   the device tree), as Kettle, Thorch and Armada. ROCKNIX fixed a sideways bottom screen after 20261001
   (PR #3507: "Sway doesn't apply a transform to an output that is powered off"): check that the bottom's
   transform holds when it is turned off and on. Touch: no calibration matrix (see "The hardware": our
   device tree reports the panels' own coordinates); ROCKNIX's hwdb entry marks the touchscreens as not
   keyboards (they report `BTN_TOUCH`, which made wlroots count a keyboard in the seat). Tap all four
   corners of both screens first; if they land turned or mirrored, a udev rule with
   `LIBINPUT_CALIBRATION_MATRIX` by `ID_PATH` (`platform-a90000.i2c` top, `platform-98c000.i2c` bottom):
   `0 -1 1 1 0 0` turns +90°, `0 1 0 -1 0 1` −90°, `-1 0 1 0 -1 1` 180°. The top panel runs at 118.6 Hz
   uncalibrated (ROCKNIX PR #3489, after 20261001).
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
   machine boots fresh. A resume that hangs costs one session, not a boot loop: the kernel resets the
   swap file's hibernate signature as soon as it finds the image (`swsusp_check`), before loading it.
9. The fan: `clamshell-fan` takes it (`pwm1_enable` 3) and sets it every 3 s; tried only on a copy of
   /sys (its tests). The sensors' types starting with `cpu` or `gpu` and the chip's name `pwmfan` are
   checked in our DTB. Check on the device that the fan stops below 50 °C (Auto; ours alone, Armada's
   never stops) and starts again cleanly at 55 °C (whether it spins up from 51/255, Armada's minimum),
   speeds up under load (sysinfo shows its rpm), and that Quiet and Max change it;
   `systemctl stop clamshell-fan` leaves it at half speed. How loud Auto is at a game's usual
   temperature, and whether the hottest sensors' mean lags a GPU spike too long.
10. Autologin without a password (a handheld's): whoever holds the device has the session. sudo and ssh
    need the password, which ClamShell's Setup tab (opened on first start) offers to set (skippable).
11. The kernel image is 62 MB (ROCKNIX's 34 MB: Armada's config keeps BTF and debug symbols); Armada's
    boots from `/KERNEL` with the same config, so it fits the ABL's load window. If the `/KERNEL` path
    alone fails, this is the suspect: a build with `CONFIG_DEBUG_INFO_BTF=n` and `CONFIG_DEBUG_INFO=n`
    in `config.thor`.
12. Steam: the client isn't in the image (it is Valve's, downloaded into the user's home); its first
    start (`steam`, from the dock's pin or a Steam game) runs `steam-setup` in a terminal
    first (see "Steam" below). Its downloads and checksums were tried on 2026-10-20 (on the PC, into a
    scratch home); the client itself has never run here.

## Before the device arrives

- [x] Build the image and check it (done 2026-10-14; see above).
- [x] Settle 2: sdhci-msm is built in (checked 2026-10-14, strings in the Image; f2fs too).
- [ ] Rebuild nearer the time: a newer ROCKNIX release, current packages.
- [x] A kernel with binder, for Android (Waydroid). Since 2026-10-20 Armada's kernel (binder is in their
      config; since 2026-10-21 built by linux-thor's PKGBUILD); what follows is the ROCKNIX build it
      replaced. ROCKNIX's SM8550 config has
      `# CONFIG_ANDROID_BINDER_IPC is not set` (checked 2026-10-19, its `next` branch and tag 20261001),
      and the option is built-in only. Done 2026-10-19: a script of ours (thor-kernel.sh, gone since) built ROCKNIX's
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
      Build its package, then the image: `tools/repo/build.sh linux-thor`, `tools/image/image.sh`.
      Checked on the PC: the config matches ROCKNIX's embedded one except for binder, DMA-BUF heaps,
      hostname, initramfs, release and compiler-dependent options (clang turns on BTI and the 32-bit
      vDSO); the same 275 modules; the Thor's device tree byte-identical to the one in ROCKNIX's
      `/KERNEL`; `/KERNEL` repacked with our kernel by `bootimg.py repack --kernel`, and split back to the
      same Image and device tree. An SD image with it: built 2026-10-20 (Armada's, 7.2.6-thor). Unverified until the device: that it boots at all (a clang build of a kernel
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
- [x] `fbcon=rotate:1` on the command line, as Thorch requires, with `CONFIG_FRAMEBUFFER_CONSOLE_ROTATION`
      in `config.thor` (Armada's config lacks it, and without it the option does nothing): the console
      turned. `thor-outputs` turns both outputs 90° by itself. Keep `/KERNEL`'s command line under 512
      bytes (Armada: the ABL may not read past that).
- [x] USB controller autosuspend (`power/control=auto` on the DWC3 device), so it doesn't hold a DDR vote
      while idle (Thorch `99-thorch-sm8550-usb-autosuspend.rules`, Armada `60-armada-usb-autosuspend.rules`).
- [x] Backlight names for the Brightness tab: top `ae96000.dsi.0`, bottom `ae94000.dsi.0` (not Kettle's
      "bottom-panel").

Our kernel is Armada's (decided 2026-10-20: the furthest community work, our own patches on top once we
have the device): linux-thor's PKGBUILD builds the Armada commit pinned in it (`_armada`; following them
is `tools/repo/follow-armada.sh`, after reading what changed in their `packages/kernel` and trying the
kernel on the device), with our config lines (`config.thor`) and our patches (the `.patch` files in its
`source=`: PB-OS's `0545`, the battery firmware's own charge switches). So these come with it: the gamepad across sleep (`0508`, `0515`, `1005`–`1007`, `0534`), rumble's
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
Done in userspace instead (2026-10-20), so no patch of ours:
- [x] The stick range: Thorch sets `axis-range = <1024>` on the gamepad node (its `dts-patches/0007`);
      Armada's device tree doesn't (they calibrate through rsinput's module parameters); without either,
      games see about 73 % of the sticks' travel (±1024 of the driver's ±1408). Ours: the same
      parameters, `axis_*_min=-1024 axis_*_max=1024`, in `/etc/modprobe.d/thor-rsinput.conf`
      (thor-system). Check with evtest that the corners reach the axes' ends.
- [x] Armada's `system-sleep/50-armada-powerbutton-suspend` (no wake by the power key with the lid
      closed): ClamShell turns the power key's wakeup off as the lid closes and on as it opens
      (`clamshell-system::lid`). `40-armada-wake-ledger` (what woke it): our `thor-wake-log` hook,
      `/var/log/thor-wake.log`.
- [x] The thermal sensors' wake interrupts off: Kettle's `60-kettle-tsens-nowake.rules` as ours,
      `75-thor-tsens.rules` (Thorch patches it out instead, `0003-thermal-qcom-tsens-skip-ayn-thor-uplow-wake-irq`).

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

1. Boot Android, update its firmware, note the versions (Android build, bootloader). Charge. How
   Android holds its charge limit (80 %, added by an AYN update; and Direct Power Supply, its bypass):
   with root, at the limit on the charger, the battery's `constant_charge_current` (or the downstream
   driver's equivalents under `/sys/class/power_supply/battery/`) and `current_now`: if the current
   goes to 0, Android holds it as our default does (`tuning::Hold`); if not, it sends the firmware a
   request mainline's qcom_battmgr doesn't, worth finding for a kernel patch.
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
   Which way it boots: with `EFI/BOOT/BOOTAA64.EFI` on the card, every bootloader we know of (U-Boot,
   ROCKNIX's ABL 1.2, OpenABL after its BLS entries) takes the UEFI way (systemd-boot) before `/KERNEL`,
   and nobody has run systemd-boot on the Thor; the way Thorch, Pocknix and Armada boot is `/KERNEL`. If
   the UEFI way shows nothing, rename `EFI` on the card's FAT partition to `EFI.disabled` (on the PC; as
   Armada's image does) to make the ABL take `/KERNEL`. Under OpenABL set "ABL mode: Verbose" to see why
   an entry fails. A kernel panic reboots after 30 s (`panic=30`).
   Serial console: none known; a USB keyboard and the screen, or ssh over USB Ethernet (after setting
   a password and turning Remote login on in ClamShell's Setup tab). Each boot runs
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
8. Maintenance: set a password (ClamShell's Setup tab; the Maintenance app asks it for sudo); install
   Claude Code (`curl -fsSL https://claude.ai/install.sh | bash`) and log in; then a first `/maintain`
   from the dock's Maintenance app (it updates through pacman; nothing of this repository is on the
   device but its packages).
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

Ours: `steam-setup` (thor-session), after Thorch's installer for Arch Linux ARM, run as the user on the
first start of `steam` (thor-session too, in a terminal):
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
      bottom-left round one should be Home (BTN_MODE). Also confirm the face buttons' codes (`evtest`):
      PB-OS found rsinput sending the top one (X) as BTN_WEST and the left one (Y) as BTN_NORTH, an Xbox
      pad's letters by place as xpad's, so ClamShell swaps X and Y as they come in for rsinput as for
      xpad (its log: "by letter (xpad, rsinput)"); bottom (B) BTN_SOUTH, right (A) BTN_EAST. If the top
      one turns out to send BTN_NORTH, drop rsinput from `pad::device::letter_codes`. And that games
      see them as expected through the virtual
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
- [ ] Battery: capacity, charging, power draw in sysinfo; the charge limit setting. Two ways to hold
      it, both in ClamShell (`tuning::Hold`): by default the charge current, as Kettle's kettle-powerd
      (0 from the limit until 5 % below it; ClamShell's log says "charging held" and "charging on"),
      since Kettle found SM8550's battery firmware never answers the end threshold's request (its patch
      `1190`; a firmware matter, not the kernel's). The other, the firmware's own threshold
      (`charge_control_end_threshold`; with Armada's patch `0904` a refused write fails, and ClamShell's
      log says so once): `CLAMSHELL_CHARGE_LIMIT=threshold`. The third, the battery firmware's own
      switches, which AYN's Android driver writes (`usb_charge_now`, `limit_capacity_charge`: PB-OS found
      them in the Thor's ADSP; their kernel patch `0545`, in linux-thor, puts them on
      `/sys/bus/auxiliary/devices/pmic_glink.power-supply.0/` as `retroid_charge_now` and
      `retroid_limit_capacity`; untried on a Thor): `CLAMSHELL_CHARGE_LIMIT=firmware`, its fixed 80 % for
      80 and charge separation for Bypass (90 stays the current's). To switch: `systemctl --user
      set-environment CLAMSHELL_CHARGE_LIMIT=…`, then `systemctl --user restart clamshell` (to keep it:
      that line in `~/.config/environment.d/`); its log says which at start. Try each at 80 and Bypass
      with the charger in: the level stops at 80, the battery's `current_now` drops to about 0. Firmware
      ones hold through sleep too, and if one works it may be what Android's own limit is. UPower
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

- [ ] ClamShell runs as a user service, in no logind session of its own: polkit's and logind's "the user's
      active session" has to resolve to tty1's autologin for these to work: the Setup tab setting a
      password (pkexec, polkit's `subject.active`), the brightness slider (logind's SetBrightness), sleep
      from its menu (logind's Suspend). Try each.
- [ ] The first device installed from an image made before thor-system had 10-thor-keep-modules.hook
      (none so far): its first kernel update can't keep the running kernel's modules (pacman reads
      pre-transaction hooks before installing the package that brings them): reboot right after it.
- [ ] A kernel update on the device: `pacman -U` a second linux-thor, then the card has the new kernel
      ("thor-boot: done"), the old release's modules are still there until the next boot
      (`/usr/lib/modules`), and gone after it (thor-old-modules). Then the FAT partition's room
      (`df -h` of it with KERNEL, KERNEL.new, Image and the initramfs): if there is room, a second loader
      entry with the previous kernel (`Image.prev`, its initramfs) for a kernel that boots and then hangs.

- [ ] Booted: the first check says `device tree: AYN Thor` and which way it booted (UEFI or `/KERNEL`),
      and `fbcon rotated` passes (the console reads upright).
- [ ] Touch: all four corners of both screens land where tapped (known issue 4).
- [ ] Wi-Fi with the device's own address (`ip link`: the same as the kernel's, not a random one), as
      Armada's and Kettle's NetworkManager config; the fan and battery drivers loaded at boot
      (thor-drivers.service: Kettle saw them late otherwise); the lid heeded 5 s after boot or waking
      (`HoldoffTimeoutSec`, Kettle's); `aplay` goes through PipeWire (pipewire-alsa).
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

## If it happens

What to do about each problem we know of from the other projects, before trying anything new. Patches
named pbos-NNNN are PB-OS's (github.com/project-barry/pb-os, `external-and-mods/kernel-sm8650/sm8550`);
ours go into linux-thor's `source=`.

Boot
- Nothing on the screen by the UEFI way: rename `EFI` on the card's FAT partition to `EFI.disabled` (on
  the PC; as Armada's image) so the ABL boots `/KERNEL`. Under OpenABL: "ABL mode: Verbose".
- `/KERNEL` alone fails (UEFI boots): the 62 MB Image; `CONFIG_DEBUG_INFO_BTF=n`, `CONFIG_DEBUG_INFO=n` in
  `config.thor`.
- ROCKNIX's ABL doesn't find the card: partition 1's type to Microsoft basic data (ROCKNIX's), in
  `tools/image/thor-image.sh`.
- A hang with nothing shown: `panic=30` reboots it; `earlycon=efifb` on the loader entry's options line
  (UEFI way) shows the kernel from its start.
- SD card errors (CRC, tuning) in the journal: ROCKNIX's `sdhci-caps-mask = <0x3 0x0>` on `mmc@8804000`
  back (a dts patch of ours; Armada drops it, Thorch reverted SDR104).
- A resume from hibernate hangs: costs that session only (the signature is reset when found); then
  `SleepOperation=suspend` in our logind drop-in until it is fixed.

Screens and touch
- Both screens upside down: `transform 270` instead of 90 in `thor-outputs` (KWin's Rotated270 is the
  same turn named the other way).
- Touch turned or mirrored: a udev rule with `LIBINPUT_CALIBRATION_MATRIX` by `ID_PATH` (known issue 4
  has the matrices).
- The bottom screen sideways after it was off: ROCKNIX PR #3507 (sway doesn't apply a transform to an
  output that is off): apply it again when ClamShell turns the output on.
- Bottom touch stuck after sleep even with pbos-0630: `edt,retain-power-in-suspend` on its node
  (Thorch's dts-patches/0005, Kettle's device tree).
- The session dies at start: `journalctl --user -u 'wayland-wm@*'`; `WLR_RENDERER=pixman` in
  `~/.config/uwsm/env` to get a screen; the GPU's firmware errors in `journalctl -k`.
- The bottom brightness slider: pbos-0620's range is ours, 0-4095; another range is one number in it.

Controls
- X and Y swapped: rsinput out of `pad::device::letter_codes` (ClamShell).
- The sticks feel steppy: `rsinput.mcu_frame=10,4` (AYN's Android) or `40,7` in
  `/etc/modprobe.d/`, against ours, 6,2 (thor-0002).
- The sticks don't reach their corners, or pass them: the range in `thor-rsinput.conf` (PB-OS found the
  real travel about ±740-1000 of 1408 per unit; ROCKNIX's `gamepadcalibration`).
- The pad dead after waking ("failed to resume: error -110"): Kettle's `kettle-rsinput` system-sleep
  hook, which binds the serdev device again.
- Rumble still a click, or too strong: the hv-haptics node's `qcom,vmax-mv` (thor-0001; Armada's RP6
  patch also sets the effects' waveforms).

Power and sleep
- GPU hangs ("OOB set GPU_SET" timeouts, GMU faults): ROCKNIX's, Thorch's and Pocknix's
  `cpu0/cpuidle/state1/disable`, or PB-OS's `sm8550-irq-affinity` (GPU interrupts on the big cores,
  no `irqaffinity=` on the command line); Armada fixed it in the kernel (0620-0622, ours).
- It wakes by itself: `/var/log/thor-wake.log` names the interrupt.
- Deep sleep, to compare with s2idle: `/etc/systemd/sleep.conf.d/deep.conf` with `[Sleep]` and
  `MemorySleepMode=deep` (pbos-0505 and pbos-0547 are for it). Compare the battery over an hour asleep,
  the wakes, charging asleep (PB-OS: about 1 A asleep in deep sleep, 4.7 A awake), and that it comes
  back.
- High drain asleep: PB-OS's `sm8550-sleep` also moves audio streams to a null sink and suspends the
  sinks (an open stream keeps the audio DSP up); the stick lights.
- A USB device gone after sleep, or a reset on waking with something plugged in while asleep:
  PB-OS's 0540 (xHCI reset on resume) and 0539 (dwc3 role change deferred across sleep).
- UFS errors after long sleeps: PB-OS's note on `spm_lvl` (their `73-sm8550-ufs-sleep.rules`); Armada's
  0533.
- The charge limit doesn't hold: the other `CLAMSHELL_CHARGE_LIMIT` ways (see "Battery"); with
  `firmware`, whether `retroid_charge_now` survives unplugging and plugging in again.
- The fan stays at its boot speed: `systemctl status thor-drivers clamshell-fan`, `lsmod | grep pwm`.
- The Setup tab's password: "Not authorized" in ClamShell's log: drop `subject.local && subject.active`
  from `50-thor.rules`.

Sound and the rest
- No sound under U-Boot's way: the card is `ayn-AYNOdin2` there (thor-ucm gives it the Thor's profile).
- The mic: PB-OS found capturing through a PipeWire loopback on it killed PipeWire.
- Sound gone once the motion sensors started: the ADSP serves both; start the sensors after the sound
  card is up (PB-OS's `sm8550-sensors`).
- A DisplayPort monitor freezes the device (Armada #529): `status = "disabled"` on `&mdss_dp0` in a dts
  patch of ours until Armada fixes it.
- A DRM lease (a VR headset, gamescope per screen) leaves a thread stuck when it ends: PB-OS's 0610.

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
- Our tools: `tools/vm/vm.py` (virtual Thor, x86-64 and aarch64; `packages`, `thor-image`),
  `tools/repo/` (building, indexing and publishing the packages), `tools/image/thor-image.sh`,
  `packages/` (the PKGBUILDs), `clamshell/tests/nested-sway.sh`
