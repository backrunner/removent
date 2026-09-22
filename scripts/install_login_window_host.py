#!/usr/bin/env python3
"""Explicit administrator install of a signed, isolated LoginWindow host.
Usage: sudo python3 scripts/install_login_window_host.py APP USER_DATA [PORT]
Never invoked by ordinary application launch or by unattended tests.
"""
import json
import ctypes
import os
from pathlib import Path
import plistlib
import shutil
import stat
import subprocess
import sys
import tempfile

ROOT = Path('/Library/Application Support/Removent/LoginWindow')
PLIST = Path('/Library/LaunchAgents/com.alkinum.removent.loginwindow.plist')


def read_regular(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise ValueError('Expected a regular file')
        with os.fdopen(fd, 'rb', closefd=False) as f:
            data = f.read(1_048_577)
        if len(data) > 1_048_576:
            raise ValueError('Configuration file too large')
        return data
    finally:
        os.close(fd)


def trusted_snapshot(data):
    peers = json.loads(data)
    if not isinstance(peers, list):
        raise ValueError('Invalid peer store')
    peers = [p for p in peers if p.get('trusted') is True]
    if not any(p.get('granted_caps', {}).get('video') and p.get('granted_caps', {}).get('input') for p in peers):
        raise ValueError('Pair and approve at least one video/input controller before installation')
    return json.dumps(peers, indent=2).encode()


def safe_parents(path):
    library = ctypes.CDLL('/usr/lib/libSystem.B.dylib', use_errno=True)
    library.acl_get_file.argtypes = [ctypes.c_char_p, ctypes.c_int]
    library.acl_get_file.restype = ctypes.c_void_p
    library.acl_free.argtypes = [ctypes.c_void_p]
    for parent in [path, *path.parents]:
        if not parent.exists():
            continue
        st = parent.lstat()
        if stat.S_ISLNK(st.st_mode) or st.st_uid != 0 or st.st_mode & 0o022:
            raise ValueError('System installation paths must be root owned and not group/world writable')
        acl = library.acl_get_file(os.fsencode(parent), 0x100)
        if acl:
            library.acl_free(acl)
            raise ValueError('System installation parents must not have extended ACLs')
        if ctypes.get_errno() != 2:  # ENOENT means no extended ACL on macOS.
            raise ValueError('Could not verify system installation ACL')


def publish_install(staged, root, plist, definition):
    """Publish a complete plist without replacing another installation."""
    temporary = None
    moved = False
    published = False
    try:
        fd, name = tempfile.mkstemp(prefix='.removent-loginwindow-', dir=plist.parent)
        temporary = Path(name)
        with os.fdopen(fd, 'wb') as f:
            plistlib.dump(definition, f)
            f.flush()
            os.fchmod(f.fileno(), 0o644)
            os.fsync(f.fileno())
        if root.exists():
            raise FileExistsError('System host installation already exists')
        os.rename(staged, root)
        moved = True
        # Same-filesystem hard link gives atomic, no-overwrite publication. The
        # global LaunchAgent never observes a partial plist or missing bundle.
        os.link(temporary, plist)
        published = True
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
        if moved and not published:
            shutil.rmtree(root)


def main():
    if os.geteuid() != 0 or len(sys.argv) not in (3, 4):
        raise SystemExit(__doc__)
    app = Path(sys.argv[1]).resolve(strict=True)
    source = Path(sys.argv[2]).resolve(strict=True)
    port = int(sys.argv[3]) if len(sys.argv) == 4 else 48688
    if not 1 <= port <= 65535:
        raise ValueError('Invalid port')
    if ROOT.exists() or PLIST.exists():
        raise ValueError('An installation already exists; disable/uninstall it before replacing it')
    subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', '-R',
                    'identifier "com.alkinum.removent" and anchor apple generic', str(app)], check=True)
    subprocess.run(['/usr/sbin/spctl', '--assess', '--type', 'execute', str(app)], check=True)
    # Snapshot only known data files, never source code, configuration paths or
    # launch arguments supplied by a normal user. Failed validation publishes nothing.
    files = {
        'identity/device.key': read_regular(source / 'identity/device.key'),
        'identity/device.crt': read_regular(source / 'identity/device.crt'),
        'peers.json': trusted_snapshot(read_regular(source / 'peers.json')),
        'settings.toml': (f'device_name = "Removent Login Window"\nhost_port = {port}\n'
                          'host_enabled = true\npaired_only = true\nwindow_server_capture = true\n'
                          'vnc_enabled = false\nupdate_check_enabled = false\n').encode(),
    }
    if (source / 'relay-host.toml').exists():
        files['relay-host.toml'] = read_regular(source / 'relay-host.toml')
    safe_parents(ROOT.parent)
    safe_parents(PLIST.parent)
    ROOT.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
    staged = Path(tempfile.mkdtemp(prefix='.login-window-', dir=ROOT.parent))
    try:
        os.chmod(staged, 0o755)
        bundle = staged / 'Removent.app'
        subprocess.run(['/usr/bin/ditto', str(app), str(bundle)], check=True)
        subprocess.run(['/bin/chmod', '-RN', str(staged)], check=True)
        # Recheck the actual copied code, closing source mutation during copy.
        subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', '-R',
                        'identifier "com.alkinum.removent" and anchor apple generic', str(bundle)], check=True)
        for base, dirs, names in os.walk(bundle):
            for item in [Path(base), *(Path(base) / n for n in names)]:
                if item.is_symlink():
                    raise ValueError('System host bundle must not contain symlinks')
                st = item.stat()
                os.chown(item, 0, 0)
                os.chmod(item, st.st_mode & ~0o022)
            if any((Path(base) / d).is_symlink() for d in dirs):
                raise ValueError('System host bundle must not contain directory symlinks')
        data = staged / 'data'
        data.mkdir(mode=0o700)
        for name, content in files.items():
            path = data / name
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, 'wb') as f:
                f.write(content)
        definition = {
            'Label': 'com.alkinum.removent.loginwindow',
            'AssociatedBundleIdentifiers': ['com.alkinum.removent'],
            'LimitLoadToSessionType': ['LoginWindow'],
            'ProgramArguments': [str(ROOT / 'Removent.app/Contents/MacOS/removentd'), '--login-window'],
            'RunAtLoad': True, 'KeepAlive': True, 'ThrottleInterval': 10,
            'ProcessType': 'Interactive', 'ExitTimeOut': 10,
        }
        publish_install(staged, ROOT, PLIST, definition)
    except Exception:
        if staged.exists():
            shutil.rmtree(staged)
        raise
    print('Installed for the next LoginWindow session; no logout or restart was requested.')
    print('This is a separate admin-managed trusted-device snapshot. Refresh/revoke it explicitly.')
    print('Enable window_server_capture = true in the normal user host settings for lock-screen testing.')
    print('Signed TCC setup and real lock/logout acceptance are still required; see docs/macos-unlock.md.')


if __name__ == '__main__':
    main()
