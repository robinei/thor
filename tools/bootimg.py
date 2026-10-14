#!/usr/bin/env python3
"""Android boot images, header version 0: what the Thor's ABL boots (ROCKNIX's /KERNEL). A header page,
then the kernel, the ramdisk and an optional second stage, each padded to the page size. The kernel blob
of ROCKNIX's is a gzip'd arm64 Image with the device trees appended (the ABL picks the matching one).

  bootimg.py info IMAGE
  bootimg.py repack IMAGE --ramdisk FILE --cmdline "..." -o OUT     (same kernel, addresses and page size)

Standard library only.
"""

import argparse
import hashlib
import struct
import sys

MAGIC = b"ANDROID!"
# magic, kernel size/addr, ramdisk size/addr, second size/addr, tags addr, page size, header version,
# os version, name, cmdline, id, extra cmdline.
HEADER = struct.Struct("<8s10I16s512s32s1024s")


def read(path):
    data = open(path, "rb").read()
    fields = HEADER.unpack_from(data)
    magic, ksize, kaddr, rsize, raddr, ssize, saddr, tags, page, version, osver, name, cmdline, _id, extra = fields
    if magic != MAGIC or version != 0:
        sys.exit(f"{path}: not an Android boot image of header version 0")

    def section(offset, size):
        return data[offset:offset + size]

    def pages(size):
        return (size + page - 1) // page * page

    kernel_at = page
    ramdisk_at = kernel_at + pages(ksize)
    second_at = ramdisk_at + pages(rsize)
    return {
        "kernel": section(kernel_at, ksize), "ramdisk": section(ramdisk_at, rsize), "second": section(second_at, ssize),
        "kernel_addr": kaddr, "ramdisk_addr": raddr, "second_addr": saddr, "tags_addr": tags, "page": page,
        "os_version": osver, "name": name,
        "cmdline": (cmdline.rstrip(b"\0") + extra.rstrip(b"\0")).decode(),
    }


def write(image, path):
    page = image["page"]
    cmdline = image["cmdline"].encode()
    if len(cmdline) > 512 + 1024 - 2:
        sys.exit("command line too long")
    # The id: SHA-1 over each section and its size (as mkbootimg makes it).
    sha = hashlib.sha1()
    for part in ("kernel", "ramdisk", "second"):
        sha.update(image[part])
        sha.update(struct.pack("<I", len(image[part])))
    header = HEADER.pack(
        MAGIC, len(image["kernel"]), image["kernel_addr"], len(image["ramdisk"]), image["ramdisk_addr"],
        len(image["second"]), image["second_addr"], image["tags_addr"], page, 0, image["os_version"],
        image["name"], cmdline[:511], sha.digest(), cmdline[511:],
    )

    def padded(blob):
        return blob + b"\0" * (-len(blob) % page)

    with open(path, "wb") as out:
        out.write(padded(header))
        for part in ("kernel", "ramdisk", "second"):
            if image[part]:
                out.write(padded(image[part]))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="action", required=True)
    p = sub.add_parser("info")
    p.add_argument("image")
    p = sub.add_parser("repack")
    p.add_argument("image")
    p.add_argument("--ramdisk", required=True)
    p.add_argument("--cmdline", required=True)
    p.add_argument("-o", "--output", required=True)
    args = parser.parse_args()
    image = read(args.image)
    if args.action == "info":
        for key in ("kernel", "ramdisk", "second"):
            print(f"{key}: {len(image[key])} bytes")
        print(f"page size: {image['page']}; kernel at {image['kernel_addr']:#x}, ramdisk at {image['ramdisk_addr']:#x}")
        print(f"cmdline: {image['cmdline']}")
        return
    image["ramdisk"] = open(args.ramdisk, "rb").read()
    image["cmdline"] = args.cmdline
    write(image, args.output)


if __name__ == "__main__":
    main()
