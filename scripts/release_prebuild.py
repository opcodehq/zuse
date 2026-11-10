#!/usr/bin/env python3
"""Reuse a complete exact-commit prebuild, waiting for work already in flight."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
from urllib.parse import urlencode

EXPECTED = {'web-assets', 'unsigned-linux-x86_64', 'unsigned-linux-aarch64',
            'unsigned-macos-aarch64', 'unsigned-macos-x86_64', 'unsigned-windows-x86_64'}


def api(path):
    return json.loads(subprocess.check_output(['gh', 'api', path], text=True))


def complete_artifacts(repository, run_id, request):
    names = set()
    page = 1
    while True:
        result = request(f'repos/{repository}/actions/runs/{run_id}/artifacts?per_page=100&page={page}')
        names.update(item['name'] for item in result['artifacts'] if not item['expired'])
        if len(result['artifacts']) < 100:
            return EXPECTED <= names
        page += 1


def find_prebuild(repository, commit, request=api, sleep=time.sleep,
                  clock=time.monotonic, timeout=7200):
    query = urlencode({'head_sha': commit, 'event': 'push', 'branch': 'main', 'per_page': 100})
    runs = request(f'repos/{repository}/actions/workflows/prebuild.yml/runs?{query}')['workflow_runs']
    # A successful rerun may exist alongside a failed or pending attempt.
    matching = [run for run in runs if run['head_sha'] == commit and run['head_branch'] == 'main'
                and run['event'] == 'push']
    for run in matching:
        if run['conclusion'] == 'success' and complete_artifacts(repository, run['id'], request):
            return str(run['id'])
    pending = next((run for run in matching if run['status'] in
                    ('queued', 'in_progress', 'waiting', 'pending', 'requested')), None)
    if pending is None:
        return ''
    run_id = pending['id']
    print(f'Waiting for exact-commit prebuild {run_id}', flush=True)
    deadline = clock() + timeout
    while True:
        run = request(f'repos/{repository}/actions/runs/{run_id}')
        # Never accept a different commit, even if the API response changes.
        if run['head_sha'] != commit:
            raise ValueError('Prebuild commit changed while waiting')
        if run['status'] == 'completed':
            if run['conclusion'] == 'success' and complete_artifacts(repository, run_id, request):
                return str(run_id)
            print(f'Prebuild {run_id} did not produce usable artifacts; compiling in this release', flush=True)
            return ''
        if clock() >= deadline:
            # Fail rather than silently starting a second multi-hour build.
            raise TimeoutError(f'Prebuild {run_id} is still running after {timeout}s; retry after it finishes')
        sleep(30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--commit', required=True)
    args = parser.parse_args()
    run_id = find_prebuild(os.environ['GITHUB_REPOSITORY'], args.commit)
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
        output.write(f'run_id={run_id}\n')
    print(f'Prebuilt run for {args.commit}: {run_id or "none, compiling now"}')


if __name__ == '__main__':
    main()
