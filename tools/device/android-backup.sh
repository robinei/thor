#!/system/bin/sh
# Day one, in Android, before any flashing: copies the Thor's own partitions to the internal storage
# (/sdcard/thor-backup/), for copying off the device. Run it as root with AYN Android's "Run Script as
# Root" (Settings → Handheld Settings → Advanced), from wherever it was copied to. It only reads the
# partitions: each is copied with dd into a file of its name, with a SHA256SUMS beside them.
#
# Which: the bootloaders we may replace (abl, AYN's U-Boot in loader, xbl), and what AYN's factory packages
# (Thor_20260112.7z, Thor_20260206.7z) don't hold or would overwrite, the device's own data: persist (the
# packages write theirs), devinfo (the unlock state and the boot-mode switch; not in the packages), the
# radios' calibration (modemst1, modemst2, fsg, fsc), cdt, frp, keystore, secdata, uefivarstore, the
# nvdata, qpdata, vm-data and the like. Not the large logs (rawdump, logdump) nor what the packages restore
# whole (super, boot, vendor_boot...). The partitions, from the packages' rawprogram*.xml.
set -u
OUT=/sdcard/thor-backup
DEV=/dev/block/by-name
mkdir -p "$OUT" || exit 1
cd "$OUT" || exit 1

{
    echo "fingerprint: $(getprop ro.build.fingerprint)"
    echo "build: $(getprop ro.build.display.id)"
    echo "bootloader: $(getprop ro.bootloader)"
    echo "slot: $(getprop ro.boot.slot_suffix)"
    echo "serial: $(getprop ro.serialno)"
    echo "date: $(date)"
    echo
    ls -l "$DEV"
} > device.txt

for name in abl_a abl_b loader_a loader_b xbl_a xbl_b xbl_config_a xbl_config_b \
            persist devinfo modemst1 modemst2 fsg fsc cdt frp keystore secdata uefivarstore \
            nvdata1 nvdata2 qpdata1 qpdata2 vm-data vm-persist spunvm tzsc connsec dip limits limits-cdsp \
            ssd apdp apdpb qweslicstore_a qweslicstore_b multiimgoem_a multiimgoem_b splash; do
    if [ -e "$DEV/$name" ]; then
        if dd if="$DEV/$name" of="$name.img" bs=4096 2> /dev/null; then
            echo "copied $name ($(wc -c < "$name.img") bytes)"
        else
            echo "FAILED $name"
        fi
    else
        echo "not here: $name"
    fi
done | tee log.txt

sha256sum *.img > SHA256SUMS
sync
echo "done: $OUT ($(du -sh . | cut -f1)); copy the folder off the device"
