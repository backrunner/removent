import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import sys
import tomllib
import unittest

import relay_setup as setup


class RelaySetupTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name) / 'config with spaces'
        setup.private_directory(self.directory)
        self.cfg = {**setup.DEFAULTS, 'address': 'removent://relay.example:48700',
                    'allowed_cidrs': ['203.0.113.12/24', '2001:db8::/32']}
        self.tokens = dict(host='11' * 32, client='22' * 32, admin='33' * 32)

    def save(self):
        setup.write_json(self.directory / 'deployment.json', self.cfg)
        setup.write_json(self.directory / 'credentials.json', self.tokens)

    def test_bash_interactive_config_generates_credentials_and_preserves_them(self):
        command = ['bash', str(setup.ROOT / 'scripts/deploy_relay.sh'), 'configure', '--config-dir', str(self.directory)]
        # VPS, name, room, address, bind, CIDRs, host token, controller token.
        answers = '\n\n\nremovent://relay.example:48700\n\n203.0.113.0/24\n\n\n'
        first = subprocess.run(command, input=answers, text=True, capture_output=True)
        self.assertEqual(first.returncode, 0, first.stderr)
        tokens = setup.read_private(self.directory / 'credentials.json')
        self.assertEqual(len(set(tokens.values())), 3)
        for token in tokens.values():
            self.assertRegex(token, r'^[a-f0-9]{64}$')
            self.assertNotIn(token, first.stdout + first.stderr)
        second = subprocess.run(command, input='\n' * 8, text=True, capture_output=True)
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(setup.read_private(self.directory / 'credentials.json'), tokens)
        self.assertEqual(stat.S_IMODE((self.directory / 'credentials.json').stat().st_mode), 0o600)
        self.assertFalse((self.directory / '.deploy-lock').exists())

    def test_vps_render_only_mounts_hashes_and_preserves_identity(self):
        self.save()
        setup.render(self.directory)
        server = tomllib.loads((self.directory / 'runtime/server.toml').read_text())
        self.assertEqual(server['allowed_cidrs'], ['203.0.113.0/24', '2001:db8::/32'])
        self.assertEqual(server['rooms'][0]['host_token_sha256'], hashlib.sha256(bytes.fromhex(self.tokens['host'])).hexdigest())
        compose = json.loads((self.directory / 'runtime/compose.json').read_text())
        self.assertEqual(compose['services']['relay']['network_mode'], 'host')
        self.assertEqual(compose['volumes']['identity']['name'], 'removent-private-relay-identity')
        mounted = compose['services']['relay']['volumes'][0]['source']
        self.assertEqual(mounted, str(self.directory / 'runtime/server.toml'))
        for token in self.tokens.values():
            self.assertNotIn(token, (self.directory / 'runtime/server.toml').read_text())
        profile = self.directory / 'relay-client.toml'
        profile.write_text(profile.read_text().replace('host_fingerprint = ""', 'host_fingerprint = "' + 'aa' * 32 + '"'))
        setup.render(self.directory)
        self.assertEqual(tomllib.loads(profile.read_text())['host_fingerprint'], 'aa' * 32)
        self.cfg['address'] = 'removent://another.example:48700'
        self.save()
        setup.render(self.directory)
        self.assertEqual(tomllib.loads(profile.read_text())['host_fingerprint'], '')

    def test_cloudflare_config_admin_scope_and_custom_domain(self):
        self.cfg.update(backend='cloudflare', address='removent://relay.example:443', admin_allowed_cidrs=['198.51.100.1/32'], max_connections=42)
        self.save()
        setup.render(self.directory)
        worker = json.loads((self.directory / 'runtime/wrangler.json').read_text())
        self.assertEqual(worker['routes'], [{'pattern': 'relay.example', 'custom_domain': True}])
        self.assertEqual(worker['vars']['RELAY_ADMIN_ALLOWED_CIDRS'], '198.51.100.1/32')
        self.assertEqual(worker['vars']['RELAY_MAX_CONNECTIONS'], '42')
        self.assertEqual(tomllib.loads((self.directory / 'relay-host.toml').read_text())['transport'], 'websocket')
        self.cfg['admin_allowed_cidrs'] = None
        self.save(); setup.render(self.directory)
        worker = json.loads((self.directory / 'runtime/wrangler.json').read_text())
        self.assertEqual(worker['vars']['RELAY_ADMIN_ALLOWED_CIDRS'], worker['vars']['RELAY_ALLOWED_CIDRS'])
        self.cfg['admin_allowed_cidrs'] = []
        self.save(); setup.render(self.directory)
        self.assertEqual(json.loads((self.directory / 'runtime/wrangler.json').read_text())['vars']['RELAY_ADMIN_ALLOWED_CIDRS'], '')

    def test_bash_deploy_and_lifecycle_commands_use_private_config_without_secrets_in_argv(self):
        self.save()
        setup.render(self.directory)
        bindir = Path(self.tmp.name) / 'bin'
        bindir.mkdir()
        log = Path(self.tmp.name) / 'calls.jsonl'
        # These stubs exercise command orchestration only, not Docker/Cloudflare.
        stub = f"#!{sys.executable}\n" + r'''
import hashlib, json, os, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
if name == 'python3' and not (args and args[0].endswith('relay_cloudflare.py')):
    os.execv(os.environ['TEST_REAL_PYTHON'], [os.environ['TEST_REAL_PYTHON'], *args])
if name == 'python3':
    token = Path(args[args.index('--credential-file') + 1]).read_text().strip()
    args.append('credential-sha256=' + hashlib.sha256(bytes.fromhex(token)).hexdigest())
with open(os.environ['TEST_RELAY_CALLS'], 'a') as handle:
    handle.write(json.dumps([name, *args]) + '\n')
if name == 'uname': print('Linux')
'''
        for name in ['docker', 'uname', 'node', 'npm', 'npx', 'python3']:
            path = bindir / name
            path.write_text(stub); path.chmod(0o700)
        env = {**os.environ, 'PATH': str(bindir) + os.pathsep + os.environ['PATH'],
               'TEST_REAL_PYTHON': sys.executable, 'TEST_RELAY_CALLS': str(log)}
        command = ['bash', str(setup.ROOT / 'scripts/deploy_relay.sh')]
        def run(action):
            result = subprocess.run([*command, action, '--config-dir', str(self.directory)], env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
        for action in ['deploy', 'stop', 'start', 'status', 'logs']:
            run(action)
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        self.assertTrue(any('--force-recreate' in call for call in calls))
        self.assertTrue(any(call[-1] == 'stop' for call in calls))
        self.assertTrue(any(call[-1] == 'start' for call in calls))
        self.assertTrue((self.directory / 'deployed.json').exists())
        # A separate Cloudflare deployment leaves admission stopped, reuses its
        # previous admin credential for the pre-update stop, and can start later.
        self.directory = Path(self.tmp.name) / 'cloudflare'
        setup.private_directory(self.directory)
        self.cfg.update(backend='cloudflare', address='removent://relay.example:443')
        self.save(); setup.render(self.directory)
        run('deploy')
        old_admin = self.tokens['admin']
        self.tokens['admin'] = '44' * 32
        self.save()
        for action in ['deploy', 'start', 'stop', 'status']:
            run(action)
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        wrangler = [call for call in calls if call[0] == 'npx']
        self.assertEqual(len(wrangler), 2)
        self.assertTrue(all('--config' in call and '--secrets-file' in call for call in wrangler))
        controls = [call for call in calls if call[0] == 'python3']
        self.assertEqual([call[2] for call in controls], ['stop', 'stop', 'stop', 'start', 'stop', 'status'])
        for token in [*self.tokens.values(), old_admin]: self.assertNotIn(token, log.read_text())
        expected = lambda token: 'credential-sha256=' + hashlib.sha256(bytes.fromhex(token)).hexdigest()
        self.assertEqual([call[-1] for call in controls[:3]], [expected(old_admin), expected(old_admin), expected(self.tokens['admin'])])

    @unittest.skipUnless(os.environ.get('REMOVENT_COMPOSE_VALIDATE') == '1', 'Docker Compose v2 schema validation runs in Linux CI')
    def test_generated_compose_schema(self):
        self.save(); setup.render(self.directory)
        subprocess.run(['docker', 'compose', '-f', str(self.directory / 'runtime/compose.json'), 'config', '--quiet'], check=True, timeout=20)

    def test_invalid_credentials_cidrs_and_addresses_fail_before_writing(self):
        for cidr in ['1.2.3.4/33', '::/129', '1.2.3.4', 'localhost/32', '::ffff:1.2.3.4/95']:
            with self.assertRaises(ValueError):
                setup.validate({**self.cfg, 'allowed_cidrs': [cidr]}, self.tokens)
        for address in ['wss://relay.example:443', 'removent://user@relay.example:443', 'removent://relay.example', 'removent://relay.example:443/path']:
            with self.assertRaises(ValueError):
                setup.validate({**self.cfg, 'address': address}, self.tokens)
        for tokens in [{**self.tokens, 'host': 'secret'}, {**self.tokens, 'client': self.tokens['host']}]:
            with self.assertRaises(ValueError):
                setup.validate(self.cfg, tokens)
        self.assertEqual(setup.cidrs(['::ffff:203.0.113.0/120']), ['203.0.113.0/24'])

    def test_private_files_symlinks_and_deployed_identity_are_guarded(self):
        self.save()
        secret = self.directory / 'credentials.json'
        secret.chmod(0o644)
        with self.assertRaises(ValueError): setup.render(self.directory)
        secret.chmod(0o600)
        target = self.directory / 'target'
        target.write_text('untouched')
        secret.unlink(); secret.symlink_to(target)
        with self.assertRaises(OSError): setup.read_private(secret)
        self.assertEqual(target.read_text(), 'untouched')
        setup.write_json(self.directory / 'deployed.json', {k: self.cfg[k] for k in ('backend', 'name', 'address')})
        setup.check_deployment(self.directory, self.cfg)
        with self.assertRaises(ValueError): setup.check_deployment(self.directory, {**self.cfg, 'name': 'another'})


if __name__ == '__main__':
    unittest.main()
