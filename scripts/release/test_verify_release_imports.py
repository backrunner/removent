"""Release entry points must work without the caller's repository on sys.path."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ReleaseImportsTests(unittest.TestCase):
    def test_standalone_verifier_loads_cloudkit_dependency(self):
        directory = Path(__file__).resolve().parent
        program = ('import runpy, sys; sys.path.insert(0, sys.argv[1]); '
                   'module = runpy.run_path(sys.argv[2]); '
                   'assert callable(module["verify_cloud_sync"])')
        environment = dict(os.environ)
        environment.pop('PYTHONPATH', None)
        with tempfile.TemporaryDirectory() as working_directory:
            subprocess.run([sys.executable, '-c', program, str(directory),
                            str(directory / 'verify_release.py')],
                           cwd=working_directory, env=environment, check=True,
                           capture_output=True, text=True)


if __name__ == '__main__':
    unittest.main()
