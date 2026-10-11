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
        # Use one identity for the desktop and all helpers, just like releases.
        # Keeping an older host certificate would perpetuate mixed TCC identities.
        host = environment.get('REMOVENT_LOCAL_SIGNING_IDENTITY')
        if host:
            environment['REMOVENT_LOCAL_HOST_SIGNING_IDENTITY'] = host
    primary = environment.get('REMOVENT_LOCAL_SIGNING_IDENTITY')
    host = environment.get('REMOVENT_LOCAL_HOST_SIGNING_IDENTITY')

    def certificate(selector):
        matches = [digest for digest, name in identities if selector in (digest, name)]
        if len(matches) != 1:
            raise RuntimeError('Local signing identity is unavailable or ambiguous')
        return matches[0]

    if primary and host and certificate(primary) != certificate(host):
        raise RuntimeError('Desktop and RemoventHost must use the same signing identity')
    if not environment.get('REMOVENT_CLOUDKIT_PROFILE'):
        profile = destination / 'Contents/Helpers/RemoventSync.app/Contents/embedded.provisionprofile'
        if profile.is_file():
            environment['REMOVENT_CLOUDKIT_PROFILE'] = str(profile)
    return environment
