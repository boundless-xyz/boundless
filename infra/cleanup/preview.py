#!/usr/bin/env python3
"""Verify the reviewed cleanup targets and print a plan. Never execute deletion steps."""
import argparse
import concurrent.futures
import json
from pathlib import Path
import subprocess

MANIFEST = Path(__file__).with_name('targets.json')
READS = {
    ('sts', 'get-caller-identity'), ('s3api', 'head-bucket'), ('s3api', 'head-object'),
    ('s3api', 'get-bucket-versioning'), ('s3api', 'get-bucket-notification-configuration'),
    ('efs', 'describe-file-systems'), ('efs', 'describe-mount-targets'),
    ('ecs', 'describe-services'), ('ecs', 'list-tasks'), ('events', 'describe-rule'),
    ('events', 'list-targets-by-rule'), ('rds', 'describe-db-instances'),
    ('rds', 'describe-db-snapshots'), ('redshift', 'describe-clusters'),
    ('redshift', 'describe-cluster-snapshots'), ('kinesis', 'describe-stream-summary'),
    ('elbv2', 'describe-load-balancers'),
}
ACCOUNTS = {'boundless-dev': '751442549745', 'boundless-prod': '632745187633',
            'boundless-staging': '245178712747', 'boundless-ops': '968153779208'}
APPROVED_TARGETS = {
    ('boundless-dev', 'pote-distributor-11155111-schedule-rule-d284873'),
    ('boundless-dev', 'pote333-og-random-requestor-8453-schedule-rule-5e1f3b3'),
    ('boundless-dev', 'pote-order-slasher-11155111'),
    ('boundless-dev', 'pote999-order-stream-serv'),
    ('boundless-dev', 'pote999-order-stream-rds'),
    ('boundless-dev', 'pote999-order-stream-84532-telem-v3'),
    ('boundless-dev', 'pote999-order-stream-84532-telem-completions'),
    ('boundless-dev', 'pote999-order-stream-84532-telem-evaluations'),
    ('boundless-dev', 'pote999-order-stream-84532-telem-heartbeats'),
    ('boundless-dev', 'pote999-order-stream-lb'),
    ('boundless-prod', 'telemetry-snapshot-prod-167000-04-24-26'),
    ('boundless-prod', 'telemetry-snapshot-prod-8453-04-24-26'),
    ('boundless-prod', 'telemetry-snapshot-prod-84532-04-24-26'),
    ('boundless-staging', 'telemetry-snapshot-staging-167000-04-24-26'),
    ('boundless-staging', 'telemetry-snapshot-staging-84532-04-24-26'),
    ('boundless-prod', 'prod-11155111-order-stream-rds-snapshot'),
    ('boundless-prod', 'prod-84532-order-stream-rds-snapshot'),
    ('boundless-prod', 'prod-8453-order-stream-rds-snapshot'),
    ('boundless-prod', 'l-prod-84532-order-stream-84532-telem-v4-final'),
    ('boundless-prod', 'l-prod-11155111-order-stream-11155111-telem-v4-final'),
}


def aws(profile, args):
    if tuple(args[:2]) not in READS:
        raise ValueError('Only explicitly allowed read operations can be executed')
    result = subprocess.run(['aws', '--profile', profile, '--region', 'us-west-2',
                             *args, '--output', 'json'], check=True, capture_output=True, text=True)
    return json.loads(result.stdout) if result.stdout.strip() else None


def validate(manifest):
    seen = set()
    for target in manifest['resources']:
        if target['profile'] not in ACCOUNTS or target['id'] in seen:
            raise ValueError('Unknown account or duplicate target')
        seen.add(target['id'])
        # Names and identities are reviewed individually; shared/ambiguous resources stay out.
        if (target['profile'], target['id']) not in APPROVED_TARGETS:
            raise ValueError('Unapproved cleanup target')
        for check in target['checks']:
            if tuple(check['args'][:2]) not in READS:
                raise ValueError('A verification command is not read-only')
        anchors = {target['id']}
        for check in target['checks']:
            anchors.update(strings(check['expect']))
        anchors.update('s3://' + value for value in list(anchors))
        for step in target['steps']:
            command = step.get('command') or step.get('wait_until', {}).get('command', [])
            if not anchors.intersection(command[2:]):
                raise ValueError('A cleanup step does not reference its verified target')
        if not target['ownership'] or not target['checks']:
            raise ValueError('Every target needs ownership evidence and live identity checks')
    allowed_stacks = {('slasher', 'prod-11155111'), ('slasher', 'prod-8453'),
                      ('slasher', 'prod-84532'), ('prover', 'staging-11155111'),
                      ('prover', 'staging-84532'), ('prover-cluster', 'staging-11155111')}
    for stack in manifest['managed_stacks']:
        if (stack['project'], stack['stack']) not in allowed_stacks:
            raise ValueError('Protected stack in cleanup targets')


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from strings(item)


def verify(target, reader=aws):
    for check in target['checks']:
        actual = reader(target['profile'], check['args'])
        if actual != check['expect']:
            raise ValueError(f"{target['id']}: live identity or dependencies changed; review again")
    return {k: target[k] for k in ('id', 'profile', 'ownership', 'steps')}


def preview(manifest, reader=aws):
    validate(manifest)
    profiles = {r['profile'] for r in manifest['resources']}
    profiles.update(s['profile'] for s in manifest['managed_stacks'])
    if manifest['managed_stacks']:
        profiles.add('boundless-ops')
    for profile in sorted(profiles):
        actual = reader(profile, ['sts', 'get-caller-identity', '--query', 'Account'])
        if actual != ACCOUNTS[profile]:
            raise ValueError('AWS account does not match the reviewed target account')
    for stack in manifest['managed_stacks']:
        version = reader('boundless-ops', ['s3api', 'head-object', '--bucket',
                         'boundless-pulumi-state', '--key',
                         f".pulumi/stacks/{stack['project']}/{stack['stack']}.json",
                         '--query', 'VersionId'])
        if version != stack['version']:
            raise ValueError('A reviewed Pulumi checkpoint changed; regenerate its preview')
    # Complete all identity checks before emitting any deletion plan.
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        plans = list(pool.map(lambda t: verify(t, reader), manifest['resources']))
    return {'read_only': True, 'resources': plans, 'managed_stacks': manifest['managed_stacks'],
            'deferred': manifest['deferred']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, help='Write the reviewed plan as JSON')
    args = parser.parse_args()
    result = preview(json.loads(MANIFEST.read_text()))
    text = json.dumps(result, indent=2) + '\n'
    if args.output:
        args.output.write_text(text)
        print(f"Verified {len(result['resources'])} explicit AWS targets; no changes made.")
    else:
        print(text, end='')


if __name__ == '__main__':
    main()
