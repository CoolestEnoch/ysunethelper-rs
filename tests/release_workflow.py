#!/usr/bin/env python3
"""Release validation regression tests; no GitHub credentials or network needed."""
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import tarfile
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('release_package', ROOT / 'scripts/package-release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


@contextlib.contextmanager
def working_directory(path):
    previous = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(previous)


class ReleaseChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ysunethelper-release-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.dist = self.root / 'dist'
        self.dist.mkdir()
        self.env = patch.dict(os.environ, {'YSUNETHELPER_VERSION': 'v0.1.0-test'})
        self.env.start()
        self.addCleanup(self.env.stop)

    def populate(self):
        for name, target in release.TARGETS.items():
            archive = release.archive_name(name)
            data = ('archive fixture ' + name).encode()
            (self.dist / archive).write_bytes(data)
            info = {'name': name, 'target': target, 'archive': archive,
                    'version': 'v0.1.0-test', 'sha256': hashlib.sha256(data).hexdigest()}
            (self.dist / ('ysunethelper-' + name + '.json')).write_text(json.dumps(info))

    def checksums(self):
        with working_directory(self.root):
            release.checksums()

    def test_workflow_matches_all_seventeen_targets(self):
        workflow = (ROOT / '.github/workflows/release.yml').read_text()
        rows = re.findall(r'- \{[^\n]*target: ([^,}]+),[^\n]*name: ([^,}]+)', workflow)
        actual = {name: target for target, name in rows}
        actual['openwrt-mipsel_24kc'] = 'mipsel-unknown-linux-musl'
        self.assertEqual(actual, release.TARGETS)
        self.assertEqual(len(rows), 16)
        self.assertNotIn('actions/upload-artifact', workflow)
        self.assertNotIn('actions/download-artifact', workflow)
        self.assertNotIn('--notes-file', workflow)
        config = tomllib.loads((ROOT / 'Cross.toml').read_text())
        self.assertIn('YSUNETHELPER_VERSION', config['build']['env']['passthrough'])

    def test_complete_release_has_checksums_for_all_archives_and_manifests(self):
        self.populate()
        self.checksums()
        lines = (self.dist / 'SHA256SUMS').read_text().splitlines()
        self.assertEqual(len(lines), 34)
        for line in lines:
            digest, filename = line.split('  ', 1)
            self.assertEqual(digest, hashlib.sha256((self.dist / filename).read_bytes()).hexdigest())
        self.checksums()  # Existing SHA256SUMS is safely regenerated on a rerun.
        self.assertEqual(lines, (self.dist / 'SHA256SUMS').read_text().splitlines())

    def test_incomplete_corrupt_stale_or_mislabeled_assets_prevent_publishing(self):
        self.populate()
        archive = self.dist / release.archive_name('linux-x86_64')
        original = archive.read_bytes()
        archive.write_bytes(b'corrupt')
        with self.assertRaises(AssertionError):
            self.checksums()
        archive.write_bytes(original)
        manifest = self.dist / 'ysunethelper-linux-x86_64.json'
        original_manifest = json.loads(manifest.read_text())
        for field, value in [('version', 'v0.0.1'), ('target', 'i686-unknown-linux-musl'),
                             ('archive', '../wrong.tar.gz')]:
            manifest.write_text(json.dumps(dict(original_manifest, **{field: value})))
            with self.subTest(field=field), self.assertRaises(AssertionError):
                self.checksums()
        manifest.write_text(json.dumps(original_manifest))
        extra = self.dist / 'old-release.tar.gz'
        extra.write_bytes(b'old')
        with self.assertRaises(AssertionError):
            self.checksums()
        extra.unlink()
        manifest.unlink()
        with self.assertRaises(AssertionError):
            self.checksums()

    def test_wrong_architecture_cannot_be_packaged_under_another_name(self):
        with self.assertRaises(AssertionError):
            release.package('i686-unknown-linux-musl', 'linux-x86_64')

    @unittest.skipUnless((ROOT / 'target/local-mt7621/ysunethelper-static').exists(),
                         'Optional: local MT7621 executables are not present')
    def test_real_mt7621_archives_and_linkage(self):
        # Optional local coverage uses actual ELF executables, not mocked readelf output.
        for filename in ['README.md', 'LICENSE', 'src/config/template.json']:
            target = self.root / filename
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / filename, target)
        binary = self.root / 'target/mipsel-unknown-linux-musl/release/ysunethelper'
        binary.parent.mkdir(parents=True)
        for kind, name in [('static', 'linux-mipsle-softfloat'), ('dynamic', 'openwrt-mipsel_24kc')]:
            source = ROOT / ('target/local-mt7621/ysunethelper-' + kind)
            if not source.exists():
                continue
            shutil.copyfile(source, binary)
            with working_directory(self.root):
                release.package('mipsel-unknown-linux-musl', name)
            with tarfile.open(self.dist / release.archive_name(name)) as archive:
                files = {Path(item.name).name for item in archive.getmembers()}
            self.assertEqual(files, {'ysunethelper', 'README.md', 'LICENSE', 'config.example.json'})
            with self.assertRaises(AssertionError):
                release.verify(source, 'mipsel-unknown-linux-musl', openwrt=kind == 'static')


if __name__ == '__main__':
    unittest.main(verbosity=2)
