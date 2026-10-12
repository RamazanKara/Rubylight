import hashlib, io, json, os, pathlib, shutil, struct, subprocess, tempfile, unittest, zipfile
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[2]
PWSH = shutil.which('pwsh')
NS = {'s': 'http://signpath.io/artifact-configuration/v1'}


class SigningConfiguration(unittest.TestCase):
    def test_only_our_shipped_binaries_are_selected_with_required_metadata(self):
        config = ET.parse(ROOT / '.signpath/artifact-configuration.xml')
        files = config.find('s:zip-file/s:pe-file-set', NS)
        self.assertEqual({item.attrib['path'] for item in files.findall('s:include', NS)}, {
            'butterpollo.exe', 'butterpollo-service.exe', 'Start Rubylight.exe',
            'butterpollo-performance.exe', 'butterpollo-protocol-performance.exe',
            'butterpollo-video-packet-performance.exe',
            'vulkan-layer/butterpollo_vulkan_layer.dll',
        })
        self.assertIsNotNone(files.find('s:for-each/s:authenticode-sign', NS))
        installer = ET.parse(ROOT / '.signpath/installer-artifact-configuration.xml')
        exe = installer.find('s:zip-file/s:pe-file', NS)
        self.assertEqual(exe.attrib['path'], 'butterpollo-setup-${version}.exe')
        self.assertIsNotNone(exe.find('s:authenticode-sign', NS))
        for tree, selected in [(config, files), (installer, exe)]:
            self.assertEqual(selected.attrib['product-name'], 'Rubylight')
            self.assertEqual(selected.attrib['product-version'], '${version}')
            self.assertEqual(tree.find('s:parameters/s:parameter', NS).attrib,
                             {'name': 'version', 'required': 'true'})


@unittest.skipUnless(os.name == 'nt' and PWSH, 'Windows PowerShell is required')
class Repackage(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.distribution = self.root / 'butterpollo-rust-release'
        self.distribution.mkdir()
        self.zip = self.root / 'portable.zip'
        self.installer = self.root / 'setup.exe'
        self.stub = b'original unsigned installer stub'
        self.installer.write_bytes(self.stub + b'old ZIP' + b'BPSETUP1' + struct.pack('<Q', 7))

    def run_repackage(self):
        return subprocess.run([PWSH, '-NoProfile', '-File', str(ROOT / 'rust/release/repackage.ps1'),
                               '-Distribution', str(self.distribution), '-Zip', str(self.zip),
                               '-Installer', str(self.installer)], capture_output=True, text=True)

    def test_signed_bytes_are_rehashed_and_embedded_before_installer_signing(self):
        payload = {
            'butterpollo.exe': b'host with appended signature',
            'vulkan-layer/butterpollo_vulkan_layer.dll': b'layer with appended signature',
            'libopus-0.dll': b'unchanged upstream DLL',
        }
        for path, content in payload.items():
            target = self.distribution / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        (self.distribution / 'manifest.json').write_text('[{"path":"stale","sha256":"unsigned"}]')
        result = self.run_repackage()
        self.assertEqual(result.returncode, 0, result.stderr)
        entries = json.loads((self.distribution / 'manifest.json').read_text(encoding='utf-8-sig'))
        self.assertEqual({e['path'].replace('\\', '/'): e['sha256'] for e in entries},
                         {p: hashlib.sha256(content).hexdigest() for p, content in payload.items()})
        installer = self.installer.read_bytes()
        self.assertEqual(installer[:len(self.stub)], self.stub)
        self.assertEqual(installer[-16:-8], b'BPSETUP1')
        length, = struct.unpack('<Q', installer[-8:])
        archive = installer[-16-length:-16]
        self.assertEqual(archive, self.zip.read_bytes())
        with zipfile.ZipFile(io.BytesIO(archive)) as package:
            for path, content in payload.items():
                self.assertEqual(package.read(f'butterpollo-rust-release/{path}'), content)

    def test_invalid_installer_is_rejected_before_outputs_are_changed(self):
        self.installer.write_bytes(b'not a packaged installer')
        original = self.installer.read_bytes()
        result = self.run_repackage()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.installer.read_bytes(), original)
        self.assertFalse(self.zip.exists())
        self.assertFalse((self.distribution / 'manifest.json').exists())


if __name__ == '__main__':
    unittest.main()
