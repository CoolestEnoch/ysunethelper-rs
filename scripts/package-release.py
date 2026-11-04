#!/usr/bin/env python3
"""Validate architecture/linkage before any executable can become a release asset."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import struct
import subprocess
import tarfile
import zipfile

ASSETS = {
    'windows-x86_64', 'windows-x86', 'windows-arm64',
    'macos-x86_64', 'macos-arm64',
    'linux-x86_64', 'linux-x86', 'linux-arm32', 'linux-arm64',
    'linux-riscv64', 'linux-mipsle-softfloat',
    'android-arm32', 'android-arm64', 'android-x86', 'android-x86_64',
    'android-riscv64-experimental', 'openwrt-mipsel_24kc',
}


def pe_imports(data):
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    assert data[pe:pe + 4] == b'PE\0\0', 'Not a PE executable'
    machine, sections = struct.unpack_from('<HH', data, pe + 4)
    optional_size = struct.unpack_from('<H', data, pe + 20)[0]
    optional = pe + 24
    magic = struct.unpack_from('<H', data, optional)[0]
    directory = optional + (112 if magic == 0x20b else 96)
    imports_rva = struct.unpack_from('<I', data, directory + 8)[0]
    table = optional + optional_size

    def offset(rva):
        for i in range(sections):
            virtual_size, address, size, raw = struct.unpack_from('<IIII', data, table + 40 * i + 8)
            if address <= rva < address + max(virtual_size, size):
                return raw + rva - address
        raise ValueError(f'Unmapped RVA: {rva}')

    names = []
    if imports_rva:
        entry = offset(imports_rva)
        while any(data[entry:entry + 20]):
            name = offset(struct.unpack_from('<I', data, entry + 12)[0])
            names.append(data[name:data.index(b'\0', name)].decode('ascii'))
            entry += 20
    return machine, names


def verify(binary, target, *, openwrt=False):
    data = binary.read_bytes()
    if 'windows' in target:
        machine, imports = pe_imports(data)
        expected = {'x86_64': 0x8664, 'i686': 0x14c, 'aarch64': 0xaa64}[target.split('-')[0]]
        assert machine == expected, f'Wrong PE architecture: {machine:#x}'
        forbidden = ('vcruntime', 'msvcp', 'msvcr', 'ucrtbase', 'api-ms-win-crt')
        assert not any(name.lower().startswith(forbidden) for name in imports), f'Dynamic CRT: {imports}'
        return {'linkage': 'static-msvc-crt; Windows system DLLs required', 'imports': imports}
    if 'apple' in target:
        arches = subprocess.check_output(['lipo', '-archs', str(binary)], text=True, env=dict(os.environ, LC_ALL='C')).strip()
        assert arches == ('arm64' if target.startswith('aarch64') else 'x86_64'), arches
        libraries = subprocess.check_output(['otool', '-L', str(binary)], text=True, env=dict(os.environ, LC_ALL='C')).splitlines()[1:]
        assert all(line.strip().startswith(('/usr/lib/', '/System/Library/')) for line in libraries), libraries
        return {'linkage': 'Rust statically linked; Apple system libraries/frameworks required', 'libraries': libraries}
    assert data[:4] == b'\x7fELF', 'Not an ELF executable'
    assert data[5] == 1, 'Expected little endian'
    machine = struct.unpack_from('<H', data, 18)[0]
    expected = {'x86_64': 62, 'i686': 3, 'aarch64': 183, 'armv7': 40, 'riscv64': 243, 'riscv64gc': 243, 'mipsel': 8}[target.split('-')[0]]
    assert machine == expected, f'Wrong ELF architecture: {machine}'
    bits = 1 if expected in (3, 40, 8) else 2
    assert data[4] == bits, 'Wrong ELF word size'
    program = subprocess.check_output(['readelf', '-lW', str(binary)], text=True, env=dict(os.environ, LC_ALL='C'))
    dynamic = subprocess.check_output(['readelf', '-dW', str(binary)], text=True, env=dict(os.environ, LC_ALL='C'))
    if openwrt:
        assert target == 'mipsel-unknown-linux-musl', 'Wrong OpenWrt Rust target'
        assert 'INTERP' in program and '/lib/ld-musl-mipsel-sf.so.1' in program, 'Wrong OpenWrt dynamic loader'
        needed = re.findall(r'\(NEEDED\).*?\[(.*?)\]', dynamic)
        assert {'libssl.so.3', 'libcrypto.so.3'} <= set(needed), 'OpenWrt OpenSSL must be shared, not vendored/static'
        allowed = {'libc.so', 'libgcc_s.so.1', 'libssl.so.3', 'libcrypto.so.3', 'libatomic.so.1'}
        assert set(needed) <= allowed, f'Unexpected OpenWrt shared libraries: {needed}'
        assert '(RPATH)' not in dynamic and '(RUNPATH)' not in dynamic, 'Build-host paths must not leak into runtime'
    else:
        assert 'INTERP' not in program, 'Executable still needs a dynamic loader'
        assert '(NEEDED)' not in dynamic, 'Executable still needs shared libraries'
    if target.startswith('mipsel'):
        abi = subprocess.check_output(['readelf', '-A', str(binary)], text=True, env=dict(os.environ, LC_ALL='C'))
        assert 'Soft float' in abi, f'MIPS artifact is not soft-float:\n{abi}'
    if openwrt:
        return {'linkage': 'dynamic OpenWrt musl/OpenSSL', 'sdk': '24.10.4',
                'cpu': 'MIPS32r2 / 24Kc soft-float', 'needed': needed,
                'interpreter': '/lib/ld-musl-mipsel-sf.so.1'}
    return {'linkage': 'fully static ELF (no PT_INTERP or DT_NEEDED)'}


def package(target, name):
    assert name in ASSETS, name
    binary = Path('target') / target / 'release' / ('ysunethelper.exe' if 'windows' in target else 'ysunethelper')
    info = verify(binary, target, openwrt=name == 'openwrt-mipsel_24kc')
    binary.chmod(0o755)
    dist = Path('dist')
    dist.mkdir(exist_ok=True)
    stem = 'ysunethelper-' + name
    files = [(binary, binary.name), (Path('README.md'), 'README.md'), (Path('LICENSE'), 'LICENSE'), (Path('src/config/template.json'), 'config.example.json')]
    if 'windows' in target:
        archive = dist / (stem + '.zip')
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
            for path, arc in files:
                z.write(path, stem + '/' + arc)
    else:
        archive = dist / (stem + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as t:
            for path, arc in files:
                t.add(path, arcname=stem + '/' + arc)
    info.update(target=target, name=name, archive=archive.name,
                sha256=hashlib.sha256(archive.read_bytes()).hexdigest())
    (dist / (stem + '.json')).write_text(json.dumps(info, indent=2) + '\n')


def checksums():
    dist = Path('dist')
    manifests = [json.loads(p.read_text()) for p in dist.glob('*.json')]
    assert len(manifests) == len(ASSETS), 'Incomplete or duplicate build matrix'
    assert {m['name'] for m in manifests} == ASSETS, 'Release target mismatch'
    for m in manifests:
        assert hashlib.sha256((dist / m['archive']).read_bytes()).hexdigest() == m['sha256'], m['name']
    assets = sorted(p for p in dist.iterdir() if p.name != 'SHA256SUMS')
    (dist / 'SHA256SUMS').write_text(''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n' for p in assets))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--target')
    parser.add_argument('--name')
    parser.add_argument('--checksums', action='store_true')
    args = parser.parse_args()
    if args.checksums:
        checksums()
    else:
        if not args.target or not args.name:
            parser.error('--target and --name are required')
        package(args.target, args.name)
