#!/system/bin/sh
# Day one, in Android, after android-backup.sh: writes a bootloader (OpenABL's abl_signed-SM8550.elf,
# ROCKNIX's ABL, or a backup's abl_a.img to go back) to abl_a and abl_b, and checks that both hold it.
# Run it as root with AYN Android's "Run Script as Root" (Settings → Handheld Settings → Advanced), from
# wherever it was copied to. It takes no arguments: the file is /sdcard/thor-abl/abl.elf, on the internal
# storage (/sdcard is the internal storage, not the SD card: ROCKNIX's and Armada's flash and backup
# scripts look there for files kept on the SD card, find nothing and still report done; Armada #650, #884).
# The runner shows only "completed": the report is /sdcard/thor-abl/flash.txt.
#
# The other slot first, checked, then the one in use, so that a failed write stops while the slot the
# device boots still holds the old bootloader. Each check reads the partition back past the page cache:
# the SHA-256 of its first <size of the file> bytes against the file's (as Kettle's releases.tsv records
# ABLs; the rest of the partition keeps the old bootloader's tail, which nothing reads).
set -u
DIR=/sdcard/thor-abl
FILE=$DIR/abl.elf
DEV=/dev/block/by-name
REPORT=$DIR/flash.txt

say() { echo "$*" | tee -a "$REPORT"; }
fail() { say "FAILED: $*"; exit 1; }

mkdir -p "$DIR" || exit 1
: > "$REPORT" || exit 1
say "date: $(date)"
[ -f "$FILE" ] || fail "no $FILE"
size=$(wc -c < "$FILE")
want=$(sha256sum "$FILE" | cut -d' ' -f1)
say "file: $FILE, $size bytes, sha256 $want"
[ "$(head -c 4 "$FILE" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ] || fail "$FILE is not an ELF file"

# The slot in use last.
case $(getprop ro.boot.slot_suffix) in
    _b) slots="a b" ;;
    *) slots="b a" ;;
esac
say "slot in use: $(getprop ro.boot.slot_suffix); order: $slots"

held() {
    # (Past the page cache: what the storage holds.)
    sync
    blockdev --flushbufs "$DEV/abl_$1" 2> /dev/null || say "warning: blockdev --flushbufs abl_$1 failed"
    echo 3 > /proc/sys/vm/drop_caches 2> /dev/null || say "warning: dropping the page cache failed"
    head -c "$size" "$DEV/abl_$1" | sha256sum | cut -d' ' -f1
}

for slot in $slots; do
    part=$DEV/abl_$slot
    [ -e "$part" ] || fail "no $part"
    room=$(blockdev --getsize64 "$part") || fail "abl_$slot's size unreadable"
    [ "$size" -le "$room" ] || fail "$FILE ($size bytes) is larger than abl_$slot ($room bytes)"
    before=$(held "$slot")
    if [ "$before" = "$want" ]; then
        say "abl_$slot: already holds it"
        continue
    fi
    dd if="$FILE" of="$part" bs=4096 2> /dev/null || fail "dd to abl_$slot"
    after=$(held "$slot")
    [ "$after" = "$want" ] || fail "abl_$slot holds $after after the write, not the file (before: $before)"
    say "abl_$slot: written and checked (before: $before)"
done
say "done: abl_a and abl_b hold $FILE ($want)"
