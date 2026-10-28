#!/usr/bin/env python3
"""Check stop/resume behavior with Pulumi mocks. No cloud calls."""
import concurrent.futures
import json
import os
import re
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
projects = {'og-service': 'order-generator', 'og-schedule': 'order-generator', 'load-test': 'order-generator', 'indexer': 'indexer', 'signal': 'signal', 'zeth': 'zeth-requestor', 'kailua': 'kailua-batch'}
kinds = sys.argv[1:]
if not kinds or any(kind not in projects for kind in kinds):
    sys.exit('Pass cases: ' + ', '.join(projects))

# Check the deployed stack settings as well as the mocked component controls.
for project in {projects[kind] for kind in kinds}:
    if project in ['order-generator', 'kailua-batch', 'indexer']:
        namespace = 'order-generator-base' if project == 'order-generator' else project
        key = 'MUTE_REQUEST_PRODUCER_ALARMS' if project == 'indexer' else 'STOPPED'
        for config in (root / 'infra' / project).glob('Pulumi.*.yaml'):
            if project == 'indexer' and config.name == 'Pulumi.l-prod-1.yaml':
                assert key not in config.read_text(), 'Ethereum rewards monitoring must stay active'
                continue
            assert re.search(r'^  ' + namespace + ':' + key + r': true$', config.read_text(), re.M), config
    if project == 'signal':
        config = root / 'infra/signal/Pulumi.prod-8453.yaml'
        assert 'signal:DESIRED_COUNT: "0"' in config.read_text(), config

def check(case):
    kind, mode = case
    project = root / 'infra' / projects[kind]
    env = dict(os.environ, NODE_PATH=str(project / 'node_modules'))
    lock = json.loads((project / 'package-lock.json').read_text())
    for package in ['@pulumi/pulumi', '@pulumi/aws', '@pulumi/awsx', 'typescript']:
        expected = lock['packages']['node_modules/' + package]['version']
        installed = json.loads((project / 'node_modules' / package / 'package.json').read_text())['version']
        assert installed == expected, f'{project.name}: {package} {installed}, expected {expected}'
    result = subprocess.run(['node', str(root / 'infra/test-producer-stop.cjs'), kind, mode], env=env, capture_output=True, text=True, timeout=90)
    if result.returncode:
        raise RuntimeError(kind + '/' + mode + ': ' + result.stderr)
    return json.loads(result.stdout.splitlines()[-1])

cases = [(kind, mode) for kind in kinds for mode in ['active', 'stopped', 'resumed']]
if 'kailua' in kinds:
    cases.append(('kailua', 'idle'))
with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
    results = list(pool.map(check, cases))
for kind in kinds:
    active, stopped, resumed = [r for r in results if r['kind'] == kind and r['mode'] != 'idle']
    assert active['resourceKeys'] == stopped['resourceKeys'] == resumed['resourceKeys'], kind + ': resource identities changed'
    assert active['nonControlHash'] == stopped['nonControlHash'] == resumed['nonControlHash'], kind + ': non-control resource inputs changed'
    print(kind + ': stop and resume passed; resource identities and non-control inputs retained')
print(str(len(results)) + ' Pulumi mock cases passed. No cloud calls.')
