#!/usr/bin/env python3
"""Verify deployed staging candidates and write an unsigned Safe draft. RPC reads only."""
import argparse
import importlib.util
import json
import re
from pathlib import Path
from urllib.parse import urlparse

spec = importlib.util.spec_from_file_location('staging_rehearsal', Path(__file__).with_name('rehearse-staging.py'))
staging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(staging)


class ReadOnlyRPC(staging.RPC):
    def __init__(self, url):
        parsed = urlparse(url)
        assert parsed.scheme == 'https' or (parsed.scheme == 'http' and parsed.hostname == '127.0.0.1' and parsed.port), 'HTTPS or loopback RPC required'
        self.url = url
        self.local = False
        self.block = None

    def call(self, method, params):
        assert method in staging.READS, 'read-only preparation forbids RPC mutations'
        params = list(params)
        if self.block is not None:
            index = {'eth_call': 1, 'eth_getCode': 1, 'eth_getStorageAt': 2}.get(method)
            if index is not None:
                assert params[index] in ('latest', self.block), 'mixed snapshot blocks'
                params[index] = self.block
        return super().call(method, params)


def prepare(label, url, candidates, output):
    chain_id, _, keys = staging.CHAINS[label]
    assert set(candidates) == set(keys), 'supply exactly the candidates required for this chain'
    for address in candidates.values():
        assert re.fullmatch(r'0x[0-9a-fA-F]{40}', address) and int(address, 16), 'invalid candidate address'
    rpc = ReadOnlyRPC(url)
    assert int(rpc.call('eth_chainId', []), 16) == chain_id, 'wrong RPC chain'
    block = rpc.call('eth_getBlockByNumber', ['latest', False])
    assert int(block['timestamp'], 16) < 1793836800, 'cutoff already reached'
    rpc.block = block['number']
    manifest = json.loads((staging.ROOT / 'preparation-evidence/deployment-inputs/manifest.json').read_text())
    baseline_record = json.loads((staging.ROOT / 'preparation-evidence/staging' / label / 'transaction.json').read_text())
    safe = staging.TAIKO_SAFE if label == 'taiko' else staging.SAFE
    steps = []
    for key in keys:
        target = next(v for v in manifest['targets'] if v['environment'] == 'rehearsal' and v['contract'] == key)
        proxy = target['proxy']
        old = '0x' + rpc.call('eth_getStorageAt', [proxy, staging.SLOT, 'latest'])[-40:]
        assert old.lower() == target['baseline_implementation'].lower(), 'staging baseline changed'
        old_hash = staging.keccak(rpc.call('eth_getCode', [old, 'latest']))
        reviewed = next(s for s in baseline_record['steps'] if s['proxy'].lower() == proxy.lower())
        assert old_hash == reviewed['baselineCodeHash'], 'baseline runtime changed'
        if key in ('accounting', 'mint'):
            assert rpc.address(proxy, 'owner()').lower() == safe.lower(), 'unexpected staging owner'
        else:
            assert rpc.uint(proxy, 'hasRole(bytes32,address)', '0x' + '00' * 32, safe) == 1, 'missing staging Safe admin'
        artifact_path = staging.ROOT / manifest['artifacts'][key]['path']
        assert staging.hashlib.sha256(artifact_path.read_bytes()).hexdigest() == manifest['artifacts'][key]['artifact_sha256'], 'artifact changed'
        candidate = candidates[key]
        assert candidate.lower() != old.lower(), 'candidate is the baseline'
        runtime_hash = staging.verify_runtime(rpc, proxy, json.loads(artifact_path.read_text()), candidate)
        steps.append({'contract': key, 'proxy': proxy, 'baseline': old, 'baselineCodeHash': old_hash,
                      'implementation': candidate, 'runtimeCodeHash': runtime_hash,
                      'artifact_sha256': manifest['artifacts'][key]['artifact_sha256']})
    # Retain the reviewed constructor dependencies, rather than accepting a compatible but different graph.
    fields = {'router': 'ROUTER', 'collateralTokenContract': 'COLLATERAL_TOKEN_CONTRACT', 'legacyImpl': 'LEGACY_IMPL',
              'verifier': 'VERIFIER', 'token': 'TOKEN', 'logUpdaterId': 'LOG_UPDATER_ID', 'accounting': 'ACCOUNTING',
              'mintCalculatorId': 'MINT_CALCULATOR_ID', 'tokenRewards': 'TOKEN_REWARDS'}
    for key in keys:
        target = next(v for v in manifest['targets'] if v['environment'] == 'rehearsal' and v['contract'] == key)
        for argument in target['constructor_inputs']:
            assert rpc.view(target['proxy'], fields[argument['name']] + '()').lower() == '0x' + argument['value'][2:].lower().rjust(64, '0'), 'constructor dependency changed'
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(output.name + '.tmp')
    try:
        transaction = staging.export_transaction(rpc, label, chain_id, steps, temporary, int(rpc.block, 16))
        owners = transaction['owners']
        assert len({owner.lower() for owner in owners}) == len(owners) and all(int(owner, 16) for owner in owners), 'invalid Safe owners'
        assert rpc.call('eth_getBlockByNumber', [rpc.block, False])['hash'] == block['hash'], 'snapshot reorganized'
        fixture = urlparse(url).hostname == '127.0.0.1'
        transaction.update(scope=('LOCAL FORK FIXTURE ONLY. Do not submit these candidate addresses to a live Safe.' if fixture else
                                 'Unsigned staging draft for supplied deployed candidates. Recheck nonce and candidates before collecting signatures.'),
                           local_fixture=fixture, live_submission_ready=False, snapshotBlockHash=block['hash'],
                           rpc_reads_only=True, requires_signer_confirmation=True)
        temporary.write_text(json.dumps(transaction, indent=2) + '\n')
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    return transaction


def parse_candidates(values):
    result = {}
    for value in values:
        key, address = value.split('=', 1)
        assert key not in result, 'duplicate candidate key'
        result[key] = address
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--chain', choices=list(staging.CHAINS), required=True)
    parser.add_argument('--candidate', action='append', required=True, metavar='CONTRACT=ADDRESS')
    parser.add_argument('--rpc-url')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    transaction = prepare(args.chain, args.rpc_url or staging.CHAINS[args.chain][1], parse_candidates(args.candidate), args.output)
    print(json.dumps({'saved': str(args.output), 'chainId': transaction['chainId'], 'safe': transaction['safe'],
                      'nonce': transaction['nonce'], 'safeTxHash': transaction['safeTxHash'],
                      'local_fixture': transaction['local_fixture'], 'rpc_reads_only': True}))


if __name__ == '__main__':
    main()
