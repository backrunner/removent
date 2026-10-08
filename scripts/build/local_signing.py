"""Choose local signatures without silently changing installed TCC identities."""
import re
import subprocess


def signing_authority(app):
    if not app.exists():
        return None
    result = subprocess.run(['codesign', '-d', '-vv', str(app)], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f'Cannot inspect installed signature: {app}')
    return next((line.removeprefix('Authority=') for line in result.stderr.splitlines()
                 if line.startswith('Authority=')), None)


def configure_local_signing(environment, destination):
    environment = dict(environment)
    if environment.get('APPLE_SIGNING_IDENTITY'):
        return environment
    output = subprocess.check_output(['security', 'find-identity', '-v', '-p', 'codesigning'], text=True)
    identities = re.findall(r'([A-F0-9]{40}) "([^"\n]+)"', output)

    def installed_identity(app):
        authority = signing_authority(app)
        if not authority:
            return None
        matches = [digest for digest, name in identities if name == authority]
        if len(matches) != 1:
            raise RuntimeError(f'Installed signing identity is unavailable or ambiguous: {authority}. '
                               'Set REMOVENT_LOCAL_SIGNING_IDENTITY explicitly to change it.')
        return matches[0]

    explicit = environment.get('REMOVENT_LOCAL_SIGNING_IDENTITY')
    if not explicit:
        selected = installed_identity(destination)
        if not selected:
            for kind in ('Developer ID Application', 'Apple Development'):
                matches = [digest for digest, name in identities if name.startswith(kind + ':')]
                if len(matches) == 1:
                    selected = matches[0]
                    break
        if selected:
            environment['REMOVENT_LOCAL_SIGNING_IDENTITY'] = selected
    if not environment.get('REMOVENT_LOCAL_HOST_SIGNING_IDENTITY'):
        # Screen capture can be attributed to the outer app, while control is
        # attributed to RemoventHost. Their existing certificates can differ.
        host = None if explicit else installed_identity(destination / 'Contents/Helpers/RemoventHost.app')
        host = host or environment.get('REMOVENT_LOCAL_SIGNING_IDENTITY')
        if host:
            environment['REMOVENT_LOCAL_HOST_SIGNING_IDENTITY'] = host
    return environment
