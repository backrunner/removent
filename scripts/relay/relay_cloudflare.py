#!/usr/bin/env python3
"""Manage a deployed Cloudflare relay without putting credentials in argv/URLs."""
import argparse
import json
import os
import re
import stat
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None  # Never forward the management credential to another URL.


def read_token(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        meta = os.fstat(fd)
        if not stat.S_ISREG(meta.st_mode) or meta.st_mode & 0o077:
            raise ValueError('Credential must be a private regular file (chmod 600)')
        token = os.read(fd, 128).decode().strip()
        if not re.fullmatch(r'[a-fA-F0-9]{64}', token):
            raise ValueError('Expected one 64-character token in the credential file')
        return token
    finally:
        os.close(fd)


def endpoint(base, action):
    url = urllib.parse.urlsplit(base)
    local = url.hostname in ('localhost', '127.0.0.1', '::1')
    if (url.scheme != 'https' and not (url.scheme == 'http' and local)) or not url.hostname or url.username or url.password or url.query or url.fragment or url.path not in ('', '/'):
        raise ValueError('Expected an HTTPS relay origin without path or credentials')
    if action not in ('start', 'stop', 'status'):
        raise ValueError('Unknown relay action')
    return urllib.parse.urlunsplit((url.scheme, url.netloc, '/admin/' + action, '', ''))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['start', 'stop', 'status'])
    parser.add_argument('url', help='https://removent-private-relay.example.workers.dev')
    parser.add_argument('--credential-file', required=True, help='Private file containing the admin token')
    args = parser.parse_args()
    url = endpoint(args.url, args.action)
    token = read_token(args.credential_file)
    request = urllib.request.Request(url, method='GET' if args.action == 'status' else 'POST', headers={'Authorization': 'Bearer ' + token})
    try:
        with urllib.request.build_opener(NoRedirect).open(request, timeout=90) as response:
            data = json.loads(response.read(4096))
    except urllib.error.HTTPError as error:
        raise SystemExit(f'Relay management failed (HTTP {error.code})') from None
    except urllib.error.URLError:
        raise SystemExit('Relay management connection failed') from None
    print(json.dumps({key: data.get(key) for key in ('state', 'enabled', 'running')}, ensure_ascii=False))


if __name__ == '__main__':
    main()
