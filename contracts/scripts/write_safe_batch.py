#!/usr/bin/env python3
"""Write a Safe Transaction Builder JSON batch from a forge script (via FFI).

The Solidity side passes admin-gated calls as --to/--data/--label arguments.
One --to applies to every call; repeated --to values specify a target per call.
The resulting file can be imported
directly into the Safe UI's Transaction Builder and executed as a single batch, so the
deployer EOA never needs ADMIN_ROLE. Mirrors the FFI convention of update_deployment_toml.py.
"""

import argparse
import json
import os
import time
from pathlib import Path

def build_batch(chain_id, safe, targets, data, labels, name):
    if not data:
        raise ValueError("no calls to batch")
    if len(data) != len(labels):
        raise ValueError("each --data must have a matching --label")
    if len(targets) == 1:
        targets = targets * len(data)
    if len(targets) != len(data):
        raise ValueError("supply one --to for all calls or one per --data")
    return {
        "version": "1.0",
        "chainId": str(chain_id),
        "createdAt": int(time.time() * 1000),
        "meta": {
            "name": name,
            "description": "; ".join(labels),
            "txBuilderVersion": "1.16.5",
            "createdFromSafeAddress": safe,
            "createdFromOwnerAddress": "",
        },
        "transactions": [
            {"to": target, "value": "0", "data": call,
             "contractMethod": None, "contractInputsValues": None}
            for target, call in zip(targets, data)
        ],
    }


def main():
    chain_key = os.environ.get("CHAIN_KEY", "anvil")
    parser = argparse.ArgumentParser(description="Write a Safe Transaction Builder JSON batch.")
    parser.add_argument("--chain-id", required=True, help="EVM chain id")
    parser.add_argument("--safe", required=True, help="Safe address that will execute the batch")
    parser.add_argument("--to", action="append", required=True, help="One target for all calls, or repeat per call")
    parser.add_argument("--data", action="append", default=[], help="Calldata hex (repeatable)")
    parser.add_argument("--label", action="append", default=[], help="Human label per call (repeatable)")
    parser.add_argument("--name", default=f"BoundlessRouter bootstrap ({chain_key})")
    parser.add_argument("--output", type=Path, default=Path("contracts/safe-batch") / f"router-bootstrap-{chain_key}.json")
    args = parser.parse_args()
    try:
        batch = build_batch(args.chain_id, args.safe, args.to, args.data, args.label, args.name)
    except ValueError as error:
        parser.error(str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(batch, indent=2) + "\n")
    print(str(args.output))


if __name__ == "__main__":
    main()
