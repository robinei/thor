# Game plan: when the AYN Thor arrives

The device: AYN Thor Max (16 GB / 1 TB, Snapdragon 8 Gen 2 / SM8550, 6" top + 3.92" bottom OLED).
The goal: our own minimal Arch Linux ARM with sway and the system program, booted from SD card first,
Android kept. Written 2026-10-14, before having the device; everything here is unverified on it.

## How the boot works (checked against ROCKNIX's SM8550 release 20261001)

- **The stock bootloader may already boot Linux.** ROCKNIX's Thor page: the ABL steps "are only required
  if your device doesn't have a fastboot menu to switch the Boot Mode", so newer stock firmware has a
  Linux boot mode of its own (someone on Discord: at least some Thors ship with U-Boot, hopefully all by
  now). Check this before flashing anything (day one, step 2). Unverified: what the stock Linux mode boots
  (presumably the same FAT `ROCKNIX` partition and `/KERNEL`, since ROCKNIX's images work either way), and
  whether there is really a U-Boot behind it (if so, it may read `extlinux.conf` and want an `Image` and
  device tree as files rather than an Android boot image: then the image needs a second layout).
- ROCKNIX's own image doesn't use U-Boot: its SM8550 options say `BOOTLOADER="qcom-abl"`. Qualcomm's
  loaders, then ROCKNIX's ABL (their build of the Android bootloader,
  `3rdparty/bootloader/rocknix_abl/abl_signed-SM8550.elf` in the release, with backup, flash and restore
  scripts), which finds a FAT partition labelled `ROCKNIX` on internal storage, SD or USB and boots its
  `/KERNEL`. Holding volume up at power-on picks the target, Android included.
- `/KERNEL` is an Android boot image, header version 0, page size 2048: a gzip'd arm64 `Image` with 14
  device trees appended (AYN Thor, Odin 2 / Mini / Portal, gamepad nodes; the ABL picks the matching one),
  the initramfs built into the kernel (the image's ramdisk is 5 bytes), and the command line
  `boot=LABEL=ROCKNIX disk=LABEL=STORAGE quiet rootwait console=tty0 allow_mismatched_32bit_el0
  fw_devlink.strict=1 pcie_ports=compat irqaffinity=0-2 cgroup.memory=nokmem,nosocket nosoftlockup
  usbcore.interrupt_interval_override=045e:028e:2`.
- ROCKNIX's sway has DSI-2 as the top screen and DSI-1 as the bottom one (our default too).

## Our image (`tools/vm.py --arch aarch64 thor-image`, built in the ARM virtual machine)

What goes in, and from where:

| Part | Source |
|---|---|
| Bootloader | not in the image: the stock one if it has a Linux boot mode, else ROCKNIX's ABL, flashed once (see below) |
| `/KERNEL` on FAT `ROCKNIX` (GPT name `system`, legacy-boot attribute, at 16 MiB) | ROCKNIX's kernel and device trees, unchanged; our initramfs (mkinitcpio: base udev modconf block filesystems fsck, no autodetect) and command line (`root=LABEL=THOR_ROOT rw` + ROCKNIX's SM8550 parameters), repacked by `tools/bootimg.py` |
| Kernel modules, firmware | ROCKNIX's `SYSTEM` (same release), incl. Qualcomm firmware: for this device only, not for sharing |
| Root: ext4 `THOR_ROOT` | `pacstrap` from Arch Linux ARM's mirrors (unpinned): base, sudo, mkinitcpio, NetworkManager, OpenSSH, sway, foot, Xwayland, Mesa + `vulkan-freedreno`, PipeWire, Noto, SQLite, Python, vim |
| Session | user `thor` (passwordless sudo, my ssh key), tty1 autologin into sway, sway config (DSI-2 top, DSI-1 bottom, ROCKNIX's output lines if found), the system program built natively for aarch64 as a user service, uinput rule, NetworkManager and sshd on |

Known issues and risks, to check first:

1. Our ramdisk is meant to be unpacked over the kernel's built-in initramfs, so our `/init` wins. If not,
   ROCKNIX's init runs, looks for its own `SYSTEM`, and the boot fails.
2. Early storage: if the SD controller's driver (sdhci-msm) is a module, our initramfs must carry it.
3. Audio: Qualcomm audio also needs ALSA UCM profiles and topology firmware from ROCKNIX; not copied yet.
4. Display rotation: if the top panel is natively portrait, sway needs a transform; only right if
   ROCKNIX's sway config has it in a form the build finds.
5. Gamepad: the kernel driver comes with ROCKNIX's modules; its userspace mapping (Thorch uses
   InputPlumber maps) doesn't. Check what evdev reports before relying on it.
6. Device trees: Thorch insists on exactly one Thor device tree in `/KERNEL`; we keep ROCKNIX's 14 (as
   ROCKNIX does). If the wrong one gets picked, strip to the Thor's.
7. Not done: suspend/hibernate setup, power and fan control, growing the root partition (the image is
   12 GB).
8. Test image only: passwordless sudo, sshd with a key, autologin without a password.

## Before the device arrives

- [ ] Build the image (`tools/vm.py --arch aarch64 thor-image`); check it from the host: partition table,
      `/KERNEL` header (`tools/bootimg.py info`), the root's contents.
- [ ] Settle 2 and 4 against ROCKNIX's `SYSTEM` (is sdhci-msm built in? where does ROCKNIX rotate?), and
      add the audio files for 3.
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

## Testing our system on the device

Each subsystem, then hibernate with each in use (hibernate matters more to me than suspend).

- [ ] Screens: both on, right size, orientation and position; layer surface on DSI-1.
- [ ] Touch: the bottom screen's touch on DSI-1 (sway's `map_to_output`); touchpad mode grabs it.
- [ ] Gamepad: evdev events; our virtual pad and pad mouse; per-game buttons and deadzone.
- [ ] GPU: Freedreno (sway) and Turnip (`vkcube`, a game); MangoHud frame rate in sysinfo.
- [ ] Wi-Fi and Bluetooth; sound through speakers and headphones; volume keys.
- [ ] Battery: capacity, charging, power draw in sysinfo; the charge limit setting (sysfs path?).
- [ ] Power profiles: cpufreq policies (3 clusters?), GPU devfreq, little-cores-only; the udev rule giving
      our user write access.
- [ ] Fan: is there a sysfs control?
- [ ] Suspend (ROCKNIX's deep suspend, ~3 s resume) and hibernate (swap, resume from it, every subsystem
      after resume).
- [ ] The launcher under real use: tools/vm.py's checks again, on the device.
- [ ] FEX for x86 games (32-bit Windows games are the open question).

## References

- ROCKNIX's Thor page (the Boot Mode note): <https://rocknix.org/devices/ayn/thor-sm8550/>
- ROCKNIX: <https://github.com/ROCKNIX/distribution> (SM8550 options: `BOOTLOADER="qcom-abl"`,
  `KERNEL_TARGET="Image"`), ABL: <https://github.com/ROCKNIX/abl>
- Thorch (Arch Linux ARM on ROCKNIX's Thor work, the closest model): <https://github.com/thorch-os/thorch>
- Pocknix (Arch Linux ARM for SM8250/SM8550 handhelds): <https://github.com/shuuri-labs/pocknix-os>
- Our tools: `tools/vm.py` (virtual Thor, x86-64 and aarch64; `thor-image`), `tools/thor-image.sh`,
  `tools/bootimg.py`, `sysprog/tests/nested-sway.sh`
