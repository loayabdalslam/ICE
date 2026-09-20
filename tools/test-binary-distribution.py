"""Exercise the actual network installers against a loopback release server.
Foreign platform installers are tested with download-only and a mocked uname;
this does not claim native Linux/macOS execution on the Windows test host.
"""
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import hashlib
import json
import os
import shutil
import struct
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT / 'dist/ice-binaries'
BASH = shutil.which('bash')
POWERSHELL = shutil.which('powershell')


class ReleaseHandler(SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith('/corrupt/'):
            self.path = self.path[len('/corrupt'):]
            if self.path.endswith('/ice') or self.path.endswith('/ice.exe'):
                body = b'not the published binary'
                self.send_response(200)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
        super().do_GET()

    def log_message(self, *args):
        pass


def bash_path(path):
    value = str(path).replace('\\', '/')
    return '/' + value[0].lower() + value[2:] if len(value) > 1 and value[1] == ':' else value


class DistributionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(('127.0.0.1', 0), partial(ReleaseHandler, directory=str(REPO)))
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.base = 'http://127.0.0.1:' + str(cls.server.server_port)
        cls.version = (REPO / 'LATEST').read_text().strip()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ice-distribution-test-', dir=ROOT / 'target')
        self.path = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def run_ps(self, *args, good=True):
        result = subprocess.run([POWERSHELL, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(REPO/'install.ps1'), *args], capture_output=True, text=True, errors='replace', timeout=60)
        if good:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def run_bash(self, os_name, arch, *args, good=True, script='install.sh', corrupt=False):
        mocks = self.path/'mocks'
        mocks.mkdir(exist_ok=True)
        (mocks/'uname').write_text(f'#!/usr/bin/env bash\ncase "$1" in -s) echo {os_name};; -m) echo {arch};; esac\n', encoding='utf-8', newline='\n')
        env = dict(os.environ)
        env['ICE_BASE_URL'] = self.base + ('/corrupt' if corrupt else '')
        # Pass a POSIX PATH inside Bash; don't alter the user's real shell profile.
        command = 'PATH="$1:$PATH"; export PATH; shift; exec bash "$@"'
        result = subprocess.run([BASH, '-c', command, 'ice-test', bash_path(mocks), bash_path(REPO/script), *args], env=env, capture_output=True, text=True, errors='replace', timeout=60)
        if good:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def test_manifest_and_binary_architectures(self):
        manifest = json.loads((REPO/'DISTRIBUTION.json').read_text())
        self.assertFalse(manifest['sourceIncluded'])
        self.assertEqual(len(manifest['artifacts']), 5)
        for artifact in manifest['artifacts']:
            data = (REPO/artifact['path']).read_bytes()
            self.assertEqual(hashlib.sha256(data).hexdigest(), artifact['sha256'])
            target = artifact['target']
            if target.startswith('linux'):
                self.assertEqual(data[:4], b'\x7fELF')
                self.assertEqual(data[4], 2)  # ELF64
                self.assertEqual(struct.unpack_from('<H', data, 18)[0], 183 if 'aarch64' in target else 62)
            elif target.startswith('macos'):
                self.assertEqual(data[:4], b'\xcf\xfa\xed\xfe')
                self.assertEqual(struct.unpack_from('<I', data, 4)[0], 0x0100000c if 'aarch64' in target else 0x01000007)
            else:
                self.assertEqual(data[:2], b'MZ')
                pe = struct.unpack_from('<I', data, 0x3c)[0]
                self.assertEqual(struct.unpack_from('<H', data, pe+4)[0], 0x8664)

    def test_no_application_source_or_credentials(self):
        for path in REPO.rglob('*'):
            if '.git' in path.parts or not path.is_file():
                continue
            self.assertNotIn(path.name, ['Cargo.toml', 'Cargo.lock', '.env', 'build.rs'])
            self.assertNotEqual(path.suffix, '.rs')
            self.assertNotIn('.ice', path.relative_to(REPO).parts)

    def test_windows_install_update_uninstall_preserves_user_files(self):
        dest = self.path/'Windows install with spaces'
        args = ['-BaseUrl', self.base, '-InstallDir', str(dest), '-NoPath']
        self.run_ps(*args)
        self.assertIn('ice 0.2.0', subprocess.check_output([str(dest/'ice.exe'), '--version'], text=True))
        marker = dest/'keep-my-file.txt'
        marker.write_text('user data')
        self.run_ps(*args)
        self.run_ps('-InstallDir', str(dest), '-Uninstall')
        self.assertFalse((dest/'ice.exe').exists())
        self.assertEqual(marker.read_text(), 'user data')

    def test_windows_corrupt_download_preserves_existing_binary(self):
        dest = self.path/'unchanged'
        dest.mkdir()
        existing = dest/'ice.exe'
        existing.write_bytes(b'existing installation')
        result = self.run_ps('-BaseUrl', self.base+'/corrupt', '-InstallDir', str(dest), '-NoPath', good=False)
        self.assertIn('SHA-256 mismatch', result.stderr)
        self.assertEqual(existing.read_bytes(), b'existing installation')

    def test_windows_iex_one_liner(self):
        dest = self.path/'iex-install'
        env = dict(os.environ, ICE_BASE_URL=self.base, ICE_INSTALL_DIR=str(dest))
        # DownloadOnly suppresses PATH edits while exercising the published IEX form.
        command = f"$script = (Invoke-WebRequest -UseBasicParsing '{self.base}/install.ps1').Content; & ([ScriptBlock]::Create($script)) -NoPath"
        result = subprocess.run([POWERSHELL,'-NoProfile','-Command',command],env=env,capture_output=True,text=True,errors='replace',timeout=60)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertTrue((dest/'ice.exe').exists())

    def test_unix_platform_downloads_and_arch_selection(self):
        for system, arch, target, script in [
            ('Linux','x86_64','linux-x86_64','bash/install.sh'),
            ('Linux','aarch64','linux-aarch64','install.sh'),
            ('Darwin','x86_64','macos-x86_64','mac/install.sh'),
            ('Darwin','arm64','macos-aarch64','mac/install.sh'),
        ]:
            dest = self.path/target
            self.run_bash(system,arch,'--download-only','--bin-dir',bash_path(dest),script=script)
            self.assertEqual((dest/'ice').read_bytes(), (REPO/f'releases/{self.version}/{target}/ice').read_bytes())

    def test_unix_corrupt_download_preserves_existing_binary(self):
        dest=self.path/'unix installed'
        dest.mkdir()
        (dest/'ice').write_bytes(b'old binary')
        result=self.run_bash('Linux','x86_64','--download-only','--bin-dir',bash_path(dest),corrupt=True,good=False)
        self.assertIn('SHA-256 mismatch',result.stderr)
        self.assertEqual((dest/'ice').read_bytes(),b'old binary')

    def test_unix_invalid_version_and_os_stop_early(self):
        self.run_bash('Linux','x86_64','--version','../../outside','--download-only',good=False)
        self.run_bash('Linux','x86_64','--print-target',script='mac/install.sh',good=False)
        self.run_bash('Darwin','ppc64','--print-target',good=False)

    def test_unix_piped_one_liner(self):
        mocks=self.path/'mocks'
        mocks.mkdir()
        (mocks/'uname').write_text('#!/usr/bin/env bash\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n',encoding='utf-8',newline='\n')
        dest=self.path/'piped install'
        env=dict(os.environ,ICE_BASE_URL=self.base)
        command='set -o pipefail; PATH="$1:$PATH"; export PATH; curl -fsSL "$2/install.sh" | bash -s -- --download-only --bin-dir "$3"'
        result=subprocess.run([BASH,'-c',command,'ice-test',bash_path(mocks),self.base,bash_path(dest)],env=env,capture_output=True,text=True,errors='replace',timeout=60)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertTrue((dest/'ice').exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
