#!/usr/bin/env python3
"""Observe CPU drawing and asynchronous GPU readiness for plain and SMAA Vulkan clients.
Build ClamShell's fast profile first. Requires the hardware nested test dependencies and vkcube.
GPU readiness includes queueing and event-loop latency; these are not isolated GPU execution times.
"""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'clamshell/tests'))
import nested_env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seconds', type=float, default=20)
    parser.add_argument('--render-node', type=Path)
    args = parser.parse_args()
    nested_env.BIN = ROOT / 'clamshell/target/fast/clamshell'
    n = nested_env.Nested(gpu=True, prefix='thor-drawing-')
    n.env['RUST_LOG'] = 'info,tracing=warn,clamshell_compositor::nested=debug'
    n.env['CLAMSHELL_DEVICE'] = 'generic'
    if args.render_node:
        n.env['WLR_RENDER_DRM_DEVICE'] = str(args.render_node.resolve())
    n.sway_config.write_text('output HEADLESS-1 resolution 3160x1080@60Hz\ndefault_border none\n'
                             'for_window [title="ClamShell nested"] fullscreen enable\n')
    result = {'commit': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT,text=True).strip(),
              'fixture': 'hardware nested in headless sway; 3160x1080 at 60 Hz; vkcube; bottom Dock, then Settings for SMAA',
              'meaning': 'cpu_us: drawing start through GPU submission; ready_us: start through fence observation, including queueing and wakeup latency',
              'phases': {}}
    with n:
        n.control('tab','dock')
        client = n.spawn(['vkcube','--wsi','wayland'], 'vulkan', n.nested)
        n.wait(lambda: 'opens:' in n.log(), 'Vulkan window opened')
        for phase in ['plain','smaa']:
            if phase == 'smaa':
                n.control('tab','settings')
                n.control('tap','setting','aa')
                n.control('tap','setting','aa')
            time.sleep(3)
            n.shot(phase)
            shutil.copyfile(n.directory / (phase + '.png'), args.output.with_suffix('.' + phase + '.png'))
            offset = len(n.log())
            print(f'{phase}: observing {args.seconds:g} seconds', flush=True)
            began = time.monotonic()
            time.sleep(args.seconds)
            elapsed = time.monotonic() - began
            rows = re.findall(r'nested frame timing: cpu_us=(\d+) ready_us=(\d+)', n.log()[offset:])
            if len(rows) < args.seconds * 20:
                raise RuntimeError(f'not enough drawing samples: {len(rows)}')
            result['phases'][phase] = {'seconds': elapsed, 'samples': [{'cpu_us':int(cpu),'ready_us':int(ready)} for cpu,ready in rows]}
            if client.poll() is not None:
                raise RuntimeError('Vulkan client exited during measurement')
        shutil.copyfile(n.directory / 'compositor.log',args.output.with_suffix('.log'))
        args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(f'observations: {args.output}', flush=True)


if __name__ == '__main__':
    main()
