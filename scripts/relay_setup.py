#!/usr/bin/env python3
"""Private configuration/rendering helper for deploy_relay.sh. Never source config."""
import argparse
import getpass
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import stat
import sys
import tempfile
import tomllib
import urllib.parse

ROOT = Path(__file__).resolve().parent.parent
DEFAULTS = dict(backend='vps', name='removent-private-relay', room='office',
                address='', listen_ip='0.0.0.0', allowed_cidrs=[], admin_allowed_cidrs=None,
                idle_seconds=300, max_connections=128, max_clients_per_room=4,
                max_bytes_per_second=50000000)


def private_directory(path):
    path = Path(path).expanduser().absolute()
    if path.is_symlink():
        raise ValueError('Configuration directory cannot be a symlink')
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = path.resolve()
    if path.is_relative_to(ROOT):
        raise ValueError('Store deployment configuration outside the source repository')
    meta = path.stat()
    if meta.st_uid != os.getuid() or meta.st_mode & 0o077:
        raise ValueError('Configuration directory must be owned by you with mode 0700')
    return path


def read_private(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as handle:
        meta = os.fstat(handle.fileno())
        if not stat.S_ISREG(meta.st_mode) or meta.st_uid != os.getuid() or meta.st_mode & 0o077:
            raise ValueError(f'{path.name} must be your private regular file (0600)')
        return json.load(handle)


def write_file(path, data, mode=0o600):
    if path.is_symlink():
        raise ValueError(f'Refusing symlink: {path.name}')
    fd, temporary = tempfile.mkstemp(prefix='.relay-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as handle:
            os.fchmod(handle.fileno(), mode)
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def write_json(path, value, mode=0o600):
    write_file(path, json.dumps(value, indent=2) + '\n', mode)


def cidrs(values):
    if not isinstance(values, list) or len(values) > 256:
        raise ValueError('Use a list of at most 256 CIDRs')
    output = []
    for value in values:
        if not isinstance(value, str) or '/' not in value or '%' in value:
            raise ValueError('Whitelist entries must be IPv4/IPv6 CIDRs, such as 203.0.113.0/24')
        network = ipaddress.ip_network(value, strict=False)
        # Normalize IPv4-mapped IPv6 consistently across Rust and the Worker.
        original = ipaddress.ip_address(value.split('/')[0])
        if isinstance(original, ipaddress.IPv6Address) and original.ipv4_mapped:
            if network.prefixlen < 96:
                raise ValueError('Mapped IPv4 CIDR prefix must be >= 96')
            network = ipaddress.ip_network(f'{original.ipv4_mapped}/{network.prefixlen - 96}', strict=False)
        output.append(str(network))
    return list(dict.fromkeys(output))


def endpoint(value):
    u = urllib.parse.urlsplit(value)
    if (u.scheme != 'removent' or not u.hostname or not u.port or u.username is not None
            or u.password is not None or u.path or u.query or u.fragment
            or re.search(r'[\s\\%?#]', value)):
        raise ValueError('Use removent://domain-or-IP:port, without path, query or credentials')
    try:
        host = str(ipaddress.ip_address(u.hostname))
    except ValueError:
        host = u.hostname.encode('idna').decode().lower()
        if len(host) > 253 or not all(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', part)
                                      for part in host.split('.')):
            raise ValueError('Invalid relay hostname')
    return f'removent://[{host}]:{u.port}' if ':' in host else f'removent://{host}:{u.port}'


def validate(settings, credentials):
    if set(settings) != set(DEFAULTS):
        raise ValueError('Unknown or missing deployment settings')
    cfg = dict(settings)
    if cfg['backend'] not in ('vps', 'cloudflare'):
        raise ValueError('Backend must be vps or cloudflare')
    if not isinstance(cfg['name'], str) or not re.fullmatch(r'[a-z][a-z0-9-]{0,49}', cfg['name']):
        raise ValueError('Name must use lowercase letters, digits and hyphens (1-50 characters)')
    if not isinstance(cfg['room'], str) or not re.fullmatch(r'[A-Za-z0-9_-]{1,64}', cfg['room']):
        raise ValueError('Invalid room ID')
    cfg['address'] = endpoint(cfg['address'])
    cfg['listen_ip'] = str(ipaddress.ip_address(cfg['listen_ip']))
    cfg['allowed_cidrs'] = cidrs(cfg['allowed_cidrs'])
    if cfg['admin_allowed_cidrs'] is not None:
        cfg['admin_allowed_cidrs'] = cidrs(cfg['admin_allowed_cidrs'])
    for key, low, high in [('max_connections', 1, 4096), ('max_clients_per_room', 1, 64),
                           ('max_bytes_per_second', 2048, 10000000000), ('idle_seconds', 0, 86400)]:
        if type(cfg[key]) is not int or not low <= cfg[key] <= high:
            raise ValueError(f'Invalid {key}')
    if 0 < cfg['idle_seconds'] < 60:
        raise ValueError('Idle seconds must be 0 or 60..86400')
    if cfg['backend'] == 'cloudflare':
        u = urllib.parse.urlsplit(cfg['address'])
        if u.port != 443:
            raise ValueError('Cloudflare public address must use port 443')
        try:
            ipaddress.ip_address(u.hostname)
        except ValueError:
            pass
        else:
            raise ValueError('Cloudflare requires a domain, not an IP')
        if u.hostname.endswith('.workers.dev') and not u.hostname.startswith(cfg['name'] + '.'):
            raise ValueError('workers.dev hostname must start with the deployment name')
    roles = ('host', 'client', 'admin')
    if set(credentials) != set(roles) or any(not isinstance(credentials[r], str) or not re.fullmatch(r'[a-fA-F0-9]{64}', credentials[r]) for r in roles):
        raise ValueError('Each credential must have 64 hexadecimal characters')
    tokens = {r: credentials[r].lower() for r in roles}
    if len(set(tokens.values())) != len(roles):
        raise ValueError('Host, controller and admin credentials must differ')
    return cfg, tokens


def ask(label, default=''):
    value = input(f'{label}' + (f' [{default}]' if default else '') + ': ').strip()
    return value or default


def check_deployment(directory, cfg):
    state = directory / 'deployed.json'
    if state.exists():
        old = read_private(state)
        if any(old[k] != cfg[k] for k in ('backend', 'name', 'address')):
            raise ValueError('Use a new configuration directory to change a deployed backend, name or address')


def configure(directory):
    current = directory / 'deployment.json'
    credentials = directory / 'credentials.json'
    cfg = read_private(current) if current.exists() else dict(DEFAULTS)
    tokens = read_private(credentials) if credentials.exists() else {}
    print('Removent relay setup. Blank keeps a displayed setting; CIDRs: comma separated, "all" = unrestricted.')
    cfg['backend'] = ask('Deployment (vps / cloudflare)', cfg['backend'])
    cfg['name'] = ask('Deployment name', cfg['name'])
    cfg['room'] = ask('Room ID', cfg['room'])
    cfg['address'] = ask('Public address (removent://domain-or-IP:port)', cfg['address'])
    if cfg['backend'] == 'vps':
        cfg['listen_ip'] = ask('Listen IP (use :: for IPv6)', cfg['listen_ip'])
    for key, label in [('allowed_cidrs', 'Host/controller allowed CIDRs'),
                       ('admin_allowed_cidrs', 'Cloudflare admin allowed CIDRs')]:
        if key.startswith('admin') and cfg['backend'] != 'cloudflare':
            continue
        default = 'inherit' if cfg[key] is None else (','.join(cfg[key]) or 'all')
        raw = ask(label, default)
        cfg[key] = None if raw == 'inherit' and key.startswith('admin') else ([] if raw == 'all' else cidrs([s.strip() for s in raw.split(',')]))
    if cfg['backend'] == 'cloudflare':
        cfg['idle_seconds'] = int(ask('Idle sleep seconds (0 disables)', str(cfg['idle_seconds'])))
    for role in ('host', 'client', 'admin'):
        if role == 'admin' and cfg['backend'] == 'vps':
            tokens.setdefault(role, secrets.token_hex(32))
            continue
        hint = 'keep existing; type generate to rotate' if role in tokens else 'generate random token'
        prompt = f'{role} credential (64 hex; blank = {hint}): '
        if sys.stdin.isatty():
            value = getpass.getpass(prompt).strip()
        else:
            print(prompt, end='', flush=True)
            value = sys.stdin.readline().strip()
        if value == 'generate' or (not value and role not in tokens):
            tokens[role] = secrets.token_hex(32)
        elif value:
            tokens[role] = value
    cfg, tokens = validate(cfg, tokens)
    check_deployment(directory, cfg)
    write_json(credentials, tokens)
    write_json(current, cfg)
    print(f'Configuration saved in {directory}. Raw credentials are in credentials.json (0600); not printed.')


def render(directory):
    cfg, tokens = validate(read_private(directory / 'deployment.json'), read_private(directory / 'credentials.json'))
    check_deployment(directory, cfg)
    runtime = directory / 'runtime'
    if runtime.is_symlink():
        raise ValueError('Runtime directory cannot be a symlink')
    runtime.mkdir(mode=0o700, exist_ok=True)
    if runtime.stat().st_uid != os.getuid() or runtime.stat().st_mode & 0o077:
        raise ValueError('Runtime directory must be private (0700)')
    hashes = {r: hashlib.sha256(bytes.fromhex(t)).hexdigest() for r, t in tokens.items()}
    port = urllib.parse.urlsplit(cfg['address']).port
    ip = cfg['listen_ip']
    listen = f'[{ip}]:{port}' if ':' in ip else f'{ip}:{port}'
    toml = lambda v: json.dumps(v, ensure_ascii=True)
    server = '\n'.join([
        f'listen = {toml(listen)}', 'identity_dir = "/var/lib/removent-relay"',
        *(f'{k} = {cfg[k]}' for k in ('max_connections', 'max_clients_per_room', 'max_bytes_per_second')),
        f'allowed_cidrs = {toml(cfg["allowed_cidrs"])}', '', '[[rooms]]',
        f'name = {toml(cfg["room"])}', f'host_token_sha256 = "{hashes["host"]}"',
        f'client_token_sha256 = "{hashes["client"]}"', '',
    ])
    # The unprivileged container can read only hashes from this explicit mount.
    write_file(runtime / 'server.toml', server, 0o644)
    write_json(runtime / 'compose.json', {
        'name': cfg['name'], 'services': {'relay': {
            'build': {'context': str(ROOT), 'dockerfile': 'deploy/relay/Dockerfile'},
            'image': f'{cfg["name"]}:local', 'network_mode': 'host',
            'restart': 'unless-stopped', 'read_only': True,
            'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true'],
            'stop_grace_period': '15s',
            'logging': {'driver': 'json-file', 'options': {'max-size': '10m', 'max-file': '3'}},
            'volumes': [{'type': 'bind', 'source': str(runtime / 'server.toml'),
                         'target': '/etc/removent-relay/server.toml', 'read_only': True},
                        {'type': 'volume', 'source': 'identity', 'target': '/var/lib/removent-relay'}],
        }}, 'volumes': {'identity': {'name': cfg['name'] + '-identity'}},
    })
    worker = json.loads((ROOT / 'deploy/cloudflare/wrangler.jsonc').read_text())
    worker.update({'name': cfg['name'], 'main': str(ROOT / 'deploy/cloudflare/src/index.ts'),
                   'workers_dev': True})
    worker.pop('$schema', None)
    public_host = urllib.parse.urlsplit(cfg['address']).hostname
    if not public_host.endswith('.workers.dev'):
        worker['routes'] = [{'pattern': public_host, 'custom_domain': True}]
    worker['containers'][0].update({'image': str(ROOT / 'deploy/cloudflare/Dockerfile'), 'image_build_context': str(ROOT)})
    worker['vars'] = {'RELAY_ROOM': cfg['room'], 'RELAY_IDLE_SECONDS': str(cfg['idle_seconds']),
                      'RELAY_ALLOWED_CIDRS': ','.join(cfg['allowed_cidrs']),
                      **{f'RELAY_{key.upper()}': str(cfg[key]) for key in ('max_connections', 'max_clients_per_room', 'max_bytes_per_second')},
                      'RELAY_ADMIN_ALLOWED_CIDRS': ','.join(cfg['allowed_cidrs'] if cfg['admin_allowed_cidrs'] is None else cfg['admin_allowed_cidrs'])}
    write_json(runtime / 'wrangler.json', worker)
    write_json(runtime / 'cloudflare-secrets.json', {f'RELAY_{r.upper()}_TOKEN_SHA256': v for r, v in hashes.items()})
    write_file(directory / 'admin.token', tokens['admin'] + '\n')
    for role in ('host', 'client'):
        # Re-rendering preserves the explicitly verified pins entered by the user.
        profile = directory / ('relay-host.toml' if role == 'host' else 'relay-client.toml')
        pins = {}
        if profile.exists():
            fd = os.open(profile, os.O_RDONLY | os.O_NOFOLLOW)
            with os.fdopen(fd) as handle:
                pins = tomllib.loads(handle.read())
        lines = [f'server = {toml(cfg["address"])}', f'transport = "{"quic" if cfg["backend"] == "vps" else "websocket"}"',
                 f'room = {toml(cfg["room"])}', f'token = "{tokens[role]}"']
        if cfg['backend'] == 'vps':
            # Changing destination must not reuse a pin from another relay.
            pin = pins.get('server_fingerprint', '') if pins.get('server') == cfg['address'] else ''
            lines.append(f'server_fingerprint = {toml(pin)} # Fill from verified relay startup log')
        if role == 'client':
            pin = pins.get('host_fingerprint', '') if pins.get('room') == cfg['room'] and pins.get('server') == cfg['address'] else ''
            lines.append(f'host_fingerprint = {toml(pin)} # Fill from target host: removent-cli identity')
        write_file(profile, '\n'.join(lines) + '\n')
    return cfg


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['prepare', 'configure', 'render', 'backend', 'origin', 'record-deployment'])
    parser.add_argument('directory')
    args = parser.parse_args()
    directory = private_directory(args.directory)
    if args.action == 'prepare':
        print(directory)
    elif args.action == 'configure':
        configure(directory)
    elif args.action == 'render':
        render(directory)
    else:
        cfg, tokens = validate(read_private(directory / 'deployment.json'), read_private(directory / 'credentials.json'))
        check_deployment(directory, cfg)
        if args.action == 'record-deployment':
            write_file(directory / 'deployed-admin.token', tokens['admin'] + '\n')
            write_file(directory / 'deployed-origin', cfg['address'].replace('removent://', 'https://', 1) + '\n')
            write_json(directory / 'deployed.json', {k: cfg[k] for k in ('backend', 'name', 'address')})
            return
        print(cfg['backend'] if args.action == 'backend' else cfg['address'].replace('removent://', 'https://', 1))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, TypeError, KeyError, OSError, EOFError, KeyboardInterrupt) as error:
        # Do not echo JSON parse errors or credential values.
        print('Relay setup failed: ' + (str(error) if not isinstance(error, json.JSONDecodeError) else 'Invalid JSON configuration'), file=sys.stderr)
        sys.exit(1)
