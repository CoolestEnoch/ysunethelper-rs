#!/usr/bin/env python3
"""Exercise a MIPS32r2 soft-float release on QEMU's 24Kc CPU before upload."""
import argparse
from pathlib import Path
import shlex
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--sysroot', type=Path)
    parser.add_argument('--library-path', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    runner = ['qemu-mipsel', '-cpu', '24Kc']
    if args.sysroot:
        runner += ['-L', str(args.sysroot.resolve(strict=True))]
    if args.library_path:
        runner += ['-E', 'LD_LIBRARY_PATH=' + str(args.library_path.resolve(strict=True))]
    result = subprocess.run(runner + [str(binary), 'version'], check=True,
                            capture_output=True, text=True, timeout=30)
    expected = 'ysunethelper ' + args.version
    if result.stdout.strip() != expected:
        raise SystemExit(f'Wrong embedded version: expected {expected!r}, got {result.stdout.strip()!r}')
    # Mock HTTPS requests also exercise the selected TLS backend and JSON parsing.
    # The test never sends requests to the real authentication portal.
    subprocess.run([sys.executable, 'tests/embedded_smoke.py', *runner, str(binary)],
                   check=True, timeout=60)
    subprocess.run([sys.executable, 'tests/compatibility.py', '--rust', str(binary),
                    '--rust-runner', shlex.join(runner)], check=True, timeout=240)
    print('MT7621 soft-float version, TLS and CLI checks passed')


if __name__ == '__main__':
    main()
