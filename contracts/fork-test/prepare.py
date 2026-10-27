#!/usr/bin/env python3
"""Verify deployed candidates and prepare unsigned upgrade/rollback drafts. RPC reads only."""
import argparse
import importlib.util
import json
import re
import tempfile
from pathlib import Path
from urllib.parse import urlparse

spec = importlib.util.spec_from_file_location(
    "staging_rehearsal", Path(__file__).with_name("rehearse.py")
)
staging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(staging)


class ReadOnlyRPC(staging.RPC):
    def __init__(self, url):
        parsed = urlparse(url)
        assert parsed.scheme == "https" or (
            parsed.scheme == "http" and parsed.hostname == "127.0.0.1" and parsed.port
        ), "HTTPS or loopback RPC required"
        self.url = url
        self.local = False
        self.block = None

    def call(self, method, params):
        assert method in staging.READS, "read-only preparation forbids RPC mutations"
        params = list(params)
        if self.block is not None:
            index = {"eth_call": 1, "eth_getCode": 1, "eth_getStorageAt": 2}.get(method)
            if index is not None:
                assert params[index] in ("latest", self.block), "mixed snapshot blocks"
                params[index] = self.block
        return super().call(method, params)


def prepare(label, url, candidates, output, rollback=False):
    chain_id, _, keys = staging.CHAINS[label]
    assert set(candidates) == set(
        keys
    ), "supply exactly the candidates required for this chain"
    for address in candidates.values():
        assert re.fullmatch(r"0x[0-9a-fA-F]{40}", address) and int(
            address, 16
        ), "invalid candidate address"
    rpc = ReadOnlyRPC(url)
    assert int(rpc.call("eth_chainId", []), 16) == chain_id, "wrong RPC chain"
    block = rpc.call("eth_getBlockByNumber", ["latest", False])
    rpc.block = block["number"]
    manifest = json.loads(staging.MANIFEST.read_text())
    if not rollback:
        assert (
            int(block["timestamp"], 16) < manifest["cutoff_timestamp"]
        ), "cutoff already reached"
    safe = (
        staging.FOUNDATION
        if chain_id == 1
        else staging.TAIKO_SAFE if label == "taiko" else staging.SAFE
    )
    steps = []
    for key in keys:
        target = next(
            v
            for v in manifest["targets"]
            if v["environment"] == staging.ENVIRONMENT and v["contract"] == key
        )
        proxy = target["proxy"]
        old = target["baseline_implementation"]
        candidate = candidates[key]
        current = "0x" + rpc.call("eth_getStorageAt", [proxy, staging.SLOT, "latest"])[-40:]
        assert (
            current.lower() == (candidate if rollback else old).lower()
        ), "installed candidate changed" if rollback else "baseline changed"
        old_hash = staging.keccak(rpc.call("eth_getCode", [old, "latest"]))
        assert old_hash == target["baseline_code_hash"], "baseline runtime changed"
        if key in ("accounting", "mint"):
            assert (
                rpc.address(proxy, "owner()").lower() == safe.lower()
            ), "unexpected Safe owner"
        else:
            assert (
                rpc.uint(proxy, "hasRole(bytes32,address)", "0x" + "00" * 32, safe) == 1
            ), "missing Safe admin"
        artifact_path, artifact = staging.artifact_for(manifest, key)
        assert candidate.lower() != old.lower(), "candidate is the baseline"
        runtime_hash = staging.verify_runtime(rpc, proxy, artifact, candidate)
        steps.append(
            {
                "contract": key,
                "proxy": proxy,
                "baseline": old,
                "baselineCodeHash": old_hash,
                "implementation": candidate,
                "runtimeCodeHash": runtime_hash,
                "artifact_sha256": staging.hashlib.sha256(
                    artifact_path.read_bytes()
                ).hexdigest(),
            }
        )
    # Retain the reviewed constructor dependencies, rather than accepting a compatible but different graph.
    fields = {
        "router": "ROUTER",
        "collateralTokenContract": "COLLATERAL_TOKEN_CONTRACT",
        "legacyImpl": "LEGACY_IMPL",
        "verifier": "VERIFIER",
        "token": "TOKEN",
        "logUpdaterId": "LOG_UPDATER_ID",
        "accounting": "ACCOUNTING",
        "mintCalculatorId": "MINT_CALCULATOR_ID",
        "tokenRewards": "TOKEN_REWARDS",
    }
    for key in keys:
        target = next(
            v
            for v in manifest["targets"]
            if v["environment"] == staging.ENVIRONMENT and v["contract"] == key
        )
        for argument in target["constructor_inputs"]:
            assert rpc.view(
                target["proxy"], fields[argument["name"]] + "()"
            ).lower() == "0x" + argument["value"][2:].lower().rjust(
                64, "0"
            ), "constructor dependency changed"
    output = Path(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as pending:
        temporary = Path(pending) / output.name
        rollback_steps = [
            dict(
                step,
                implementation=step["baseline"],
                runtimeCodeHash=step["baselineCodeHash"],
                upgradedImplementation=step["implementation"],
                upgradedRuntimeCodeHash=step["runtimeCodeHash"],
            )
            for step in steps
        ]
        transaction = staging.export_transaction(
            rpc, label, chain_id, rollback_steps if rollback else steps,
            temporary, int(rpc.block, 16),
            name=label + (" rollback" if rollback else " upgrades"),
        )
        drafts = [(temporary, transaction, rollback)]
        if not rollback:
            rollback_path = temporary.parent / "rollback/transaction.json"
            rollback_path.parent.mkdir()
            rollback_transaction = staging.export_transaction(
                rpc, label, chain_id, rollback_steps, rollback_path,
                int(rpc.block, 16), nonce=transaction["nonce"] + 1,
                name=label + " rollback",
            )
            drafts.append((rollback_path, rollback_transaction, True))
        owners = transaction["owners"]
        assert len({owner.lower() for owner in owners}) == len(owners) and all(
            int(owner, 16) for owner in owners
        ), "invalid Safe owners"
        assert (
            rpc.call("eth_getBlockByNumber", [rpc.block, False])["hash"]
            == block["hash"]
        ), "snapshot reorganized"
        fixture = urlparse(url).hostname == "127.0.0.1"
        for path, draft, is_rollback in drafts:
            draft.update(
                scope=(
                    "LOCAL FORK FIXTURE ONLY. Do not submit this draft to a live Safe."
                    if fixture
                    else "Unsigned Safe draft. Recheck nonce and implementations before collecting signatures."
                ),
                action="rollback" if is_rollback else "upgrade",
                local_fixture=fixture,
                live_submission_ready=False,
                snapshotBlockHash=block["hash"],
                rpc_reads_only=True,
                requires_signer_confirmation=True,
            )
            if is_rollback:
                draft["scope"] += (
                    " Execute only after the expected upgrade and a rollback decision."
                    " Restores original PoVW behavior and emissions."
                )
            if label == "taiko":
                draft["scope"] += " Taiko staging and production share this Safe and nonce on chain 167000."
                draft["scope"] += (
                    " Regenerate after any intervening Safe transaction."
                    if is_rollback
                    else " Execute staging first, then regenerate the production draft and nonce."
                )
            path.write_text(json.dumps(draft, indent=2) + "\n")
        # Check every destination before publishing; retain old drafts for error recovery.
        publications = []
        previous = {}
        for path, _, _ in drafts:
            destination = (
                output if path == temporary else output.parent / "rollback/transaction.json"
            )
            destination.parent.mkdir(parents=True, exist_ok=True)
            for source, target in [
                (path.with_name("safe-builder.json"), destination.with_name("safe-builder.json")),
                (path, destination),
            ]:
                previous[target] = target.read_bytes() if target.exists() else None
                publications.append((source, target))
        published = []
        try:
            for source, target in publications:
                source.replace(target)
                published.append((source, target))
        except OSError:
            for source, target in reversed(published):
                if previous[target] is None:
                    target.unlink()
                else:
                    source.write_bytes(previous[target])
                    source.replace(target)
            raise
    return transaction


def parse_candidates(values):
    result = {}
    for value in values:
        key, address = value.split("=", 1)
        assert key not in result, "duplicate candidate key"
        result[key] = address
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--environment", choices=["production", "staging"], required=True
    )
    parser.add_argument("--zkc-repo", type=Path, default=staging.ZKC)
    parser.add_argument("--chain", choices=list(staging.CHAINS), required=True)
    parser.add_argument(
        "--candidate", action="append", required=True, metavar="CONTRACT=ADDRESS"
    )
    parser.add_argument("--rpc-url")
    parser.add_argument(
        "--rollback", action="store_true",
        help="Refresh rollback after installation, including after cutoff; candidates must be currently installed",
    )
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    staging.configure(args.environment, args.zkc_repo)
    transaction = prepare(
        args.chain,
        args.rpc_url or staging.CHAINS[args.chain][1],
        parse_candidates(args.candidate),
        args.output,
        rollback=args.rollback,
    )
    print(
        json.dumps(
            {
                "saved": str(args.output),
                "chainId": transaction["chainId"],
                "safe": transaction["safe"],
                "nonce": transaction["nonce"],
                "safeTxHash": transaction["safeTxHash"],
                "local_fixture": transaction["local_fixture"],
                "rpc_reads_only": True,
            }
        )
    )


if __name__ == "__main__":
    main()
