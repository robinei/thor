#!/usr/bin/env python3
"""Package recovery transitions on temporary filesystems, without root, mounts or services."""
import importlib.util
import os
import sys
import re
from pathlib import Path
import subprocess
import tempfile
import unittest
import gzip
import zlib

sys.dont_write_bytecode = True
REPO = Path(__file__).resolve().parents[2]
BOOTIMG = REPO / 'packages/thor-system/root/usr/lib/thor/bootimg.py'
spec = importlib.util.spec_from_file_location('bootimg', BOOTIMG)
bootimg = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bootimg)


class Machine:
    def __init__(self):
        self.temp = tempfile.TemporaryDirectory(prefix='thor-package-test-')
        self.root = Path(self.temp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.env = {**os.environ, 'PATH': f'{self.bin}:/usr/bin:/bin'}

    def file(self, name, text=''):
        path = self.root / name.lstrip('/')
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def command(self, name, body=':'):
        path = self.file(f'bin/{name}', f'#!/bin/sh\n{body}\n')
        path.chmod(0o755)

    def run(self, source, roots, **env):
        source = re.sub('|'.join(re.escape(prefix) for prefix in roots), lambda match: f'{self.root}{match.group()}', source)
        script = self.file('script', source)
        return subprocess.run(['bash', script], env={**self.env, **env}, capture_output=True, text=True, timeout=15)

    def close(self):
        self.temp.cleanup()


class BootRecovery(unittest.TestCase):
    def machine(self):
        m = Machine()
        self.addCleanup(m.close)
        esp = m.root / 'run/thor-esp'
        m.file('run/thor-esp/loader/entries/thor.conf', 'title Thor\nlinux /thor-a/Image\ninitrd /thor-a/initramfs.img\ndevicetree /thor-a/thor.dtb\noptions root=LABEL=THOR_ROOT rw\n')
        for name in ['Image', 'initramfs.img', 'thor.dtb']:
            m.file(f'run/thor-esp/thor-a/{name}', f'old {name}')
        m.file('run/thor-esp/EFI/BOOT/BOOTAA64.EFI', 'old EFI')
        m.file('usr/lib/modules/new/Image', 'new Image')
        m.file('usr/lib/modules/new/dtbs/qcs8550-ayn-thor.dtb', 'new thor.dtb')
        m.file('usr/lib/systemd/boot/efi/systemd-bootaa64.efi', 'new EFI')
        m.file('usr/lib/thor/bootimg.py', BOOTIMG.read_text())
        helper = m.file('usr/lib/thor/thor-modules', '#!/bin/sh\nexit 0\n')
        helper.chmod(0o755)
        m.file('var/log/.keep')
        m.file('boot/.keep')
        bootimg.write({
            'kernel': gzip.compress(b'old Image') + b'old thor.dtb',
            'ramdisk': b'old initramfs.img', 'second': b'',
            'kernel_addr': 0, 'ramdisk_addr': 0, 'second_addr': 0, 'tags_addr': 0,
            'page': 4096, 'os_version': 0, 'name': b'Thor', 'cmdline': 'root=LABEL=THOR_ROOT rw',
        }, esp / 'KERNEL')
        m.command('pacman', f'echo {m.root}/usr/lib/modules/new/Image')
        m.command('findmnt', 'echo /dev/mmcblk0p2')
        m.command('lsblk', 'echo mmcblk0')
        m.command('mountpoint', 'exit 0')
        m.command('mount')
        m.command('umount')
        m.command('sync')
        m.command('mkinitcpio', f"printf 'new initramfs.img' > {m.root}/boot/initramfs-thor.img")
        m.command('mv', f'''n=$(cat {m.root}/renames 2>/dev/null || echo 0)
n=$((n+1)); echo "$n" > {m.root}/renames
[ "${{FAIL_BEFORE:-0}}" != "$n" ] || exit 73
/usr/bin/mv "$@" || exit
[ "${{FAIL_AFTER:-0}}" != "$n" ] || exit 74''')
        return m

    def run_boot(self, m, **env):
        source = (REPO / 'packages/thor-system/root/usr/bin/thor-boot').read_text()
        return m.run(source, ['/usr/lib/thor', '/usr/lib/modules/', '/boot/', '/var/log/', '/run/thor-esp', '/usr/lib/systemd/boot/efi'], **env)

    def coherent(self, m):
        esp = m.root / 'run/thor-esp'
        fields = dict(line.split(maxsplit=1) for line in (esp / 'loader/entries/thor.conf').read_text().splitlines())
        image = (esp / fields['linux'].lstrip('/')).read_text()
        generation = image.split()[0]
        self.assertIn(generation, ['old', 'new'])
        self.assertEqual((esp / fields['initrd'].lstrip('/')).read_text(), f'{generation} initramfs.img')
        self.assertEqual((esp / fields['devicetree'].lstrip('/')).read_text(), f'{generation} thor.dtb')
        abl = bootimg.read(esp / 'KERNEL')
        stream = zlib.decompressobj(16 + zlib.MAX_WBITS)
        kernel = stream.decompress(abl['kernel'])
        abl_generation = kernel.decode().split()[0]
        self.assertIn(abl_generation, ['old', 'new'])
        self.assertEqual(stream.unused_data, f'{abl_generation} thor.dtb'.encode())
        self.assertEqual(abl['ramdisk'], f'{abl_generation} initramfs.img'.encode())
        return fields['linux']

    def test_every_publication_failure_keeps_each_selected_boot_set_coherent(self):
        for when, code in [('FAIL_BEFORE', 73), ('FAIL_AFTER', 74)]:
            for step in range(1, 5):
                with self.subTest(when=when, step=step):
                    m = self.machine()
                    result = self.run_boot(m, **{when: str(step)})
                    self.assertEqual(result.returncode, code, result.stdout + result.stderr)
                    self.coherent(m)
                    result = self.run_boot(m)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual((m.root / 'run/thor-esp' / self.coherent(m).lstrip('/')).read_text(), 'new Image')

    def test_repeated_updates_alternate_slots_without_touching_selected_files(self):
        m = self.machine()
        self.assertEqual(self.coherent(m), '/thor-a/Image')
        for expected in ['/thor-b/Image', '/thor-a/Image', '/thor-b/Image']:
            result = self.run_boot(m)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(self.coherent(m), expected)


if __name__ == '__main__':
    unittest.main()
