import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import wait_for_ci


class ReleaseGateTests(unittest.TestCase):
    def test_uses_latest_successful_run_for_exact_main_push(self):
        matching = {'databaseId': 10, 'headSha': 'release-sha', 'headBranch': 'main',
                    'event': 'push', 'status': 'completed', 'conclusion': 'success'}
        runs = [matching, dict(matching, databaseId=11),
                dict(matching, databaseId=20, headSha='other-sha'),
                dict(matching, databaseId=21, event='pull_request'),
                dict(matching, databaseId=22, headBranch='feature')]
        with patch.object(wait_for_ci.subprocess, 'check_output', return_value=json.dumps(runs)):
            self.assertEqual(wait_for_ci.wait_for_workflow('release-sha', 'desktop-platforms.yml'), 11)

    def test_latest_failure_blocks_even_if_earlier_run_passed(self):
        run = {'databaseId': 10, 'headSha': 'release-sha', 'headBranch': 'main',
               'event': 'push', 'status': 'completed', 'conclusion': 'success'}
        runs = [run, dict(run, databaseId=11, conclusion='failure')]
        with patch.object(wait_for_ci.subprocess, 'check_output', return_value=json.dumps(runs)):
            with self.assertRaisesRegex(SystemExit, 'did not pass: 11'):
                wait_for_ci.wait_for_workflow('release-sha', 'desktop-platforms.yml')

    def test_all_three_gates_run_and_desktop_run_id_is_exported(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / 'output'
            with patch.dict(os.environ, GITHUB_OUTPUT=str(output)), \
                    patch.object(wait_for_ci.subprocess, 'check_output', return_value='release-sha\n'), \
                    patch.object(wait_for_ci, 'wait_for_workflow', side_effect=[1, 2, 3]) as gate:
                wait_for_ci.main()
            self.assertEqual([call.args for call in gate.call_args_list], [
                ('release-sha', workflow) for workflow in ('ci.yml', 'relay.yml', 'desktop-platforms.yml')])
            self.assertEqual(output.read_text(), 'desktop_run_id=3\n')


if __name__ == '__main__':
    unittest.main()
