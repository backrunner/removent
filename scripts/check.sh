#!/usr/bin/env bash
# Offline repository checks. Native lifecycle/CloudKit acceptance stays opt-in.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/check_layout.py
for scope in build release relay macos; do
    python3 -m unittest discover -s "scripts/$scope" -p 'test_*.py'
done
while IFS= read -r script; do bash -n "$script"; done < <(find scripts apps/tray/Tests -name '*.sh')
