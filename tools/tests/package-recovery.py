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


class RegistryRecovery(unittest.TestCase):
    def machine(self):
        m = Machine()
        self.addCleanup(m.close)
        m.file('usr/share/thor/sensors/config/fixture.json', '{}')
        m.file('usr/share/thor/sensors/sns_reg.conf', 'version=1\n')
        m.file('usr/share/thor/sensors/dsp/adsp/sns_tppe.so', 'fixture')
        m.file('sys/devices/soc0/soc_id', '603')
        m.command('udevadm')
        m.command('sync')
        generator = m.file('usr/lib/thor/sensors/sscregistrygen', f'''#!/bin/sh
for last; do :; done
printf generated >> {m.root}/generation-calls
printf config > "$last/aaa.config"
[ "${{FAIL_GENERATION:-0}}" != 1 ] || exit 73
printf marker > "$last/sh5001_0_platform.config"
printf tail > "$last/zzz.config"
''')
        generator.chmod(0o755)
        m.command('mv', '[ "${FAIL_PUBLISH:-0}" != 1 ] || exit 74\nexec /usr/bin/mv "$@"')
        return m

    def run_registry(self, m, **env):
        source = (REPO / 'packages/thor-sensors/thor-sensors').read_text()
        return m.run(source, ['/usr/share/thor/sensors', '/var/lib/thor-sensors', '/usr/lib/thor/sensors', '/sys/devices/soc0/'], **env)

    def complete(self, m):
        reg = m.root / 'var/lib/thor-sensors/registry'
        self.assertEqual(sorted(path.name for path in reg.iterdir()), ['aaa.config', 'sh5001_0_platform.config', 'zzz.config'])
        self.assertEqual((reg / 'zzz.config').read_text(), 'tail')

    def test_generation_and_publication_failures_leave_no_partial_registry(self):
        for failure in ['FAIL_GENERATION', 'FAIL_PUBLISH']:
            with self.subTest(failure=failure):
                m = self.machine()
                result = self.run_registry(m, **{failure: '1'})
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((m.root / 'var/lib/thor-sensors/registry').exists())
                self.assertEqual(list((m.root / 'var/lib/thor-sensors').glob('registry.new.*')), [])
                result = self.run_registry(m)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.complete(m)

    def test_existing_calibration_is_preserved_without_regeneration(self):
        m = self.machine()
        result = self.run_registry(m)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        m.file('var/lib/thor-sensors/registry/aaa.config', 'learned calibration')
        before = (m.root / 'generation-calls').read_text()
        result = self.run_registry(m)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((m.root / 'generation-calls').read_text(), before)
        self.assertEqual((m.root / 'var/lib/thor-sensors/registry/aaa.config').read_text(), 'learned calibration')

    def test_empty_registry_can_be_replaced_but_unknown_partial_data_is_preserved(self):
        m = self.machine()
        (m.root / 'var/lib/thor-sensors/registry').mkdir(parents=True)
        result = self.run_registry(m)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.complete(m)
        partial = self.machine()
        file = partial.file('var/lib/thor-sensors/registry/aaa.config', 'possibly learned calibration')
        result = self.run_registry(partial)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(file.read_text(), 'possibly learned calibration')
        self.assertFalse((partial.root / 'generation-calls').exists())


class Account(unittest.TestCase):
    """thor-account against a stub shadow entry: what counts as a password, and the paths it guards."""

    def machine(self, hash):
        m = Machine()
        self.addCleanup(m.close)
        m.file('hash', hash)
        m.file('password', 'current-password')
        m.command('id', 'echo thor')
        m.command('getent', f'printf "thor:%s:20000:0:99999:7:::\\n" "$(cat {m.root}/hash)"')
        m.command('chpasswd', f'IFS= read -r line; echo "$line" >> {m.root}/chpasswd; printf "%s" "${{line#thor:}}" > {m.root}/password; printf \'$6$set\' > {m.root}/hash')
        m.command('runuser', f'[ "$(tr -d "\\0")" = "$(cat {m.root}/password)" ]')
        m.command('systemctl', f'echo "$*" >> {m.root}/systemctl; case $* in *is-enabled*) exit 1 ;; esac')
        return m

    def account(self, m, *args, stdin=''):
        source = (REPO / 'packages/thor-system/root/usr/lib/thor/thor-account').read_text()
        script = m.file('script', source)
        return subprocess.run(['bash', script, *args], input=stdin, env={**m.env, 'PKEXEC_UID': '1000'},
                              capture_output=True, text=True, timeout=15)

    def log(self, m, name):
        path = m.root / name
        return path.read_text() if path.exists() else ''

    LOCKED = ['', '!', '!!', '!$6$salt$hash', '*', '!*']
    SET = ['$6$salt$hash', '$y$j9T$salt$hash']

    def test_status_reads_locked_and_starred_hashes_as_none_and_real_hashes_as_set(self):
        for hash, expected in [(h, 'none') for h in self.LOCKED] + [(h, 'set') for h in self.SET]:
            with self.subTest(hash=hash):
                result = self.account(self.machine(hash), 'status')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, f'password {expected}\nssh off\n')

    def test_first_password_is_set_without_the_current_one_and_then_reads_as_set(self):
        for hash in self.LOCKED:
            with self.subTest(hash=hash):
                m = self.machine(hash)
                result = self.account(m, 'password', stdin='new-password\n')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.log(m, 'chpasswd'), 'thor:new-password\n')
                self.assertEqual(self.account(m, 'status').stdout, 'password set\nssh off\n')

    def test_a_set_password_is_changed_only_with_the_current_one(self):
        for hash in self.SET:
            with self.subTest(hash=hash):
                m = self.machine(hash)
                result = self.account(m, 'password', stdin='new-password\n')
                self.assertEqual((result.returncode, result.stderr), (1, 'failed\n'))
                result = self.account(m, 'password', stdin='wrong-password\nnew-password\n')
                self.assertEqual((result.returncode, result.stderr), (1, 'wrong password\n'))
                self.assertEqual(self.log(m, 'chpasswd'), '')
                result = self.account(m, 'password', stdin='current-password\nnew-password\n')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.log(m, 'chpasswd'), 'thor:new-password\n')

    def test_ssh_on_needs_a_set_password(self):
        for hash in self.LOCKED:
            with self.subTest(hash=hash):
                m = self.machine(hash)
                result = self.account(m, 'ssh', 'on')
                self.assertEqual((result.returncode, result.stderr), (1, 'set a password first\n'))
                self.assertEqual(self.log(m, 'systemctl'), '')
        for hash in self.SET:
            with self.subTest(hash=hash):
                m = self.machine(hash)
                result = self.account(m, 'ssh', 'on')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.log(m, 'systemctl'), 'enable --now -q sshd\n')


class PanelCalibrationReaders(unittest.TestCase):
    """Root's readers of /var/lib/thor/panel-calibration, which the power group writes."""

    def machine(self):
        m = Machine()
        self.addCleanup(m.close)
        m.file('sys/bus/mipi-dsi/devices/dsi0/frame_len_120', '52\n')
        m.file('sys/bus/mipi-dsi/devices/dsi0/frame_len_60', '2000\n')
        m.file('secret', 'root:$6$secret:20000::::::\n')
        for command in ['systemctl', 'journalctl', 'runuser', 'loginctl', 'nmcli', 'NetworkManager', 'swapon',
                        'lsblk', 'findmnt', 'mount', 'mountpoint', 'pacman', 'sync', 'id']:
            m.command(command, 'exit 1')
        return m

    def selftest(self, m):
        source = (REPO / 'packages/thor-system/root/usr/bin/thor-selftest').read_text()
        source = re.sub('/sys/bus/mipi-dsi/|/var/lib/thor/|/var/log/', lambda match: f'{m.root}{match.group()}', source)
        script = m.file('script', source)
        result = subprocess.run(['bash', script, '--now'], env=m.env, capture_output=True, text=True, timeout=15)
        return [line for line in result.stdout.splitlines() if 'frame lengths' in line]

    def apply(self, m):
        source = (REPO / 'packages/thor-system/root/usr/lib/thor/panel-calibration').read_text()
        script = m.file('script', re.sub('/var/lib/thor/', f'{m.root}/var/lib/thor/', source))
        return subprocess.run(['sh', script, m.root / 'sys/bus/mipi-dsi/devices/dsi0'], env=m.env, capture_output=True, text=True, timeout=15)

    def test_a_symlink_or_empty_file_reads_as_uncalibrated_and_a_regular_file_as_its_values(self):
        for kind in ['symlink', 'empty', 'regular']:
            with self.subTest(kind=kind):
                m = self.machine()
                saved = m.root / 'var/lib/thor/panel-calibration'
                saved.parent.mkdir(parents=True)
                if kind == 'symlink':
                    saved.symlink_to(m.root / 'secret')
                elif kind == 'empty':
                    saved.write_text('')
                else:
                    saved.write_text('frame_len_120=53\nframe_len_60=2010\n')
                lines = self.selftest(m)
                self.assertEqual(len(lines), 1, lines)
                if kind == 'regular':
                    self.assertTrue(lines[0].endswith('(frame_len_120=53 frame_len_60=2010 )'), lines)
                else:
                    self.assertTrue(lines[0].endswith('(uncalibrated)'), lines)

    def test_the_driver_files_are_written_only_from_a_regular_file(self):
        for kind in ['symlink', 'regular']:
            with self.subTest(kind=kind):
                m = self.machine()
                values = 'frame_len_120=53\nframe_len_60=2010\n'
                saved = m.root / 'var/lib/thor/panel-calibration'
                saved.parent.mkdir(parents=True)
                if kind == 'symlink':
                    m.file('elsewhere', values)
                    saved.symlink_to(m.root / 'elsewhere')
                else:
                    saved.write_text(values)
                result = self.apply(m)
                self.assertEqual(result.returncode, 0, result.stderr)
                panel = m.root / 'sys/bus/mipi-dsi/devices/dsi0'
                expected = ('53\n', '2010\n') if kind == 'regular' else ('52\n', '2000\n')
                self.assertEqual(((panel / 'frame_len_120').read_text(), (panel / 'frame_len_60').read_text()), expected)


if __name__ == '__main__':
    unittest.main()
