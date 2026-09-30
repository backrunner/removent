#!/usr/bin/env python3
"""Require a successful main push CI run for the exact checked-out release SHA."""
import json
import subprocess
import time


def wait_for_workflow(sha, workflow):
    deadline = time.monotonic() + 3600
    while time.monotonic() < deadline:
        runs = json.loads(subprocess.check_output([
            'gh', 'run', 'list', '--repo', 'backrunner/removent',
            '--workflow', workflow, '--branch', 'main', '--commit', sha,
            '--event', 'push', '--limit', '5',
            '--json', 'databaseId,headSha,headBranch,event,status,conclusion',
        ], text=True, timeout=60))
        matches = [run for run in runs if run['headSha'] == sha
                   and run['headBranch'] == 'main' and run['event'] == 'push']
        if matches:
            run = max(matches, key=lambda item: item['databaseId'])
            if run['status'] == 'completed':
                if run['conclusion'] != 'success':
                    raise SystemExit('{} did not pass: {}'.format(workflow, run['databaseId']))
                print('Verified {} run {} for {}'.format(workflow, run['databaseId'], sha), flush=True)
                return
        print('Waiting for {} on main for {}'.format(workflow, sha), flush=True)
        time.sleep(30)
    raise SystemExit('Timed out waiting for {}; release is blocked'.format(workflow))


def main():
    sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    for workflow in ('ci.yml', 'relay.yml'):
        wait_for_workflow(sha, workflow)


if __name__ == '__main__':
    main()
