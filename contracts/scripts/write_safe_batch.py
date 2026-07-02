#!/usr/bin/env python3
"""Write a Safe Transaction Builder JSON batch from a forge script (via FFI).

The Solidity side collects the admin-gated calls it would otherwise broadcast and
passes them here as repeated --data/--label pairs. The resulting file can be imported
directly into the Safe UI's Transaction Builder and executed as a single batch, so the
deployer EOA never needs ADMIN_ROLE. Mirrors the FFI convention of update_deployment_toml.py.
"""

import argparse
import json
import os
import time
from pathlib import Path

CHAIN_KEY = os.environ.get("CHAIN_KEY", "anvil")
OUT_DIR = Path("contracts/safe-batch")

parser = argparse.ArgumentParser(description="Write a Safe Transaction Builder JSON batch.")
parser.add_argument("--chain-id", required=True, help="EVM chain id")
parser.add_argument("--safe", required=True, help="Safe address that will execute the batch")
parser.add_argument("--to", required=True, help="Target contract for every call (the router)")
parser.add_argument("--data", action="append", default=[], help="Calldata hex (repeatable)")
parser.add_argument("--label", action="append", default=[], help="Human label per call (repeatable)")
args = parser.parse_args()

if len(args.data) != len(args.label):
    raise SystemExit("each --data must have a matching --label")
if not args.data:
    raise SystemExit("no calls to batch")

transactions = [
    {
        "to": args.to,
        "value": "0",
        "data": data,
        "contractMethod": None,
        "contractInputsValues": None,
    }
    for data in args.data
]

batch = {
    "version": "1.0",
    "chainId": str(args.chain_id),
    "createdAt": int(time.time() * 1000),
    "meta": {
        "name": f"BoundlessRouter bootstrap ({CHAIN_KEY})",
        "description": "; ".join(args.label),
        "txBuilderVersion": "1.16.5",
        "createdFromSafeAddress": args.safe,
        "createdFromOwnerAddress": "",
    },
    "transactions": transactions,
}

OUT_DIR.mkdir(parents=True, exist_ok=True)
out_path = OUT_DIR / f"router-bootstrap-{CHAIN_KEY}.json"
out_path.write_text(json.dumps(batch, indent=2) + "\n")

# Printed to stdout so the forge script can echo the path.
print(str(out_path))
