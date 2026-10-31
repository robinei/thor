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


class SwapRecovery(unittest.TestCase):
    def machine(self, existing=True):
        m = Machine()
        self.addCleanup(m.close)
        m.file('etc/fstab', '# root is on the command line\n')
        m.file('sys/class/block/mmcblk0p2/partition', '2\n')
        m.file('var/lib/.keep')
        m.file('run/thor-esp/loader/entries/thor.conf', 'options root=LABEL=THOR_ROOT rw\n')
        if existing:
            m.file('swapfile', 'already-created-swap')
        m.command('findmnt', 'echo /dev/mmcblk0p2')
        m.command('lsblk', 'echo mmcblk0')
        for command in ['growpart', 'resize2fs', 'mount', 'umount', 'thor-boot', 'mkswap', 'sync']:
            m.command(command)
        m.command('mountpoint', 'exit 0')
        m.command('df', "printf 'Avail\\n32\\n'")
        m.command('fallocate', 'for last; do :; done; : > "$last"')
        m.command('filefrag', "echo '0: 0.. 0: 123..'")
        m.command('swapon', f'''if [ "$1" = --show=NAME ]; then
    [ ! -e {m.root}/active ] || echo {m.root}/swapfile
    exit 0
fi
[ "${{FAIL_SWAP:-0}}" != 1 ] || exit 73
echo activation >> {m.root}/activations
touch {m.root}/active''')
        return m

    def run_firstboot(self, m, **env):
        source = (REPO / 'packages/thor-system/root/usr/lib/thor/thor-firstboot').read_text()
        return m.run(source, ['/swapfile', '/etc/', '/sys/class/block/', '/run/thor-esp', '/var/lib/thor-firstboot'], **env)

    def complete(self, m):
        self.assertTrue((m.root / 'var/lib/thor-firstboot').exists())
        self.assertTrue((m.root / 'active').exists())
        self.assertEqual((m.root / 'etc/fstab').read_text().count(f'{m.root}/swapfile none swap'), 1)
        entry = (m.root / 'run/thor-esp/loader/entries/thor.conf').read_text()
        self.assertEqual(entry.count('resume_offset=123'), 1)

    def test_a_created_file_is_registered_and_activated_on_retry(self):
        m = self.machine()
        for _ in range(2):
            result = self.run_firstboot(m)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.complete(m)
        self.assertEqual((m.root / 'activations').read_text(), 'activation\n')

    def test_failed_activation_is_not_marked_complete_and_can_retry(self):
        m = self.machine()
        result = self.run_firstboot(m, FAIL_SWAP='1')
        self.assertEqual(result.returncode, 73, result.stdout + result.stderr)
        self.assertFalse((m.root / 'var/lib/thor-firstboot').exists())
        self.assertFalse((m.root / 'active').exists())
        result = self.run_firstboot(m)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.complete(m)

    def test_partial_swap_registration_is_replaced_as_a_complete_record(self):
        m = self.machine()
        m.file('etc/fstab', f'# root\n{m.root}/swapfile none')
        result = self.run_firstboot(m)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.complete(m)

    def test_unreadable_resume_offset_keeps_firstboot_incomplete(self):
        m = self.machine()
        m.command('filefrag', 'echo incomplete')
        result = self.run_firstboot(m)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertFalse((m.root / 'var/lib/thor-firstboot').exists())
        m.command('filefrag', "echo '0: 0.. 0: 123..'")
        result = self.run_firstboot(m)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.complete(m)

    def test_new_swap_and_insufficient_space_take_distinct_paths(self):
        m = self.machine(existing=False)
        result = self.run_firstboot(m)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.complete(m)
        low = self.machine(existing=False)
        low.command('df', "printf 'Avail\\n8\\n'")
        result = self.run_firstboot(low)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((low.root / 'swapfile').exists())
        self.assertFalse((low.root / 'active').exists())
        self.assertTrue((low.root / 'var/lib/thor-firstboot').exists())


if __name__ == '__main__':
    unittest.main()
