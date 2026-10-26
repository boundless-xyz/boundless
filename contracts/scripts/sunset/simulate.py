#!/usr/bin/env python3
"""Read-only Safe preflight and Tenderly draft generation. Never sends a transaction."""
import argparse
import base64
import json
from pathlib import Path
import re
import subprocess
import urllib.request

ZERO = "0x" + "0" * 40
SIGNATURE = "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)"

def encode(signature, *args):
    return subprocess.check_output(["cast", "calldata", signature, *map(str, args)], text=True).strip()

def rpc(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json", "User-Agent": "Boundless-sunset-readonly"})
    with urllib.request.urlopen(req, timeout=60) as response:
        result = json.load(response)
    if "error" in result:
        raise RuntimeError(f"{method}: {result['error']}")
    return result["result"]

def extract(log):
    prefix = "SUNSET_TX_JSON="
    rows = [json.loads(line.strip()[len(prefix):]) for line in log.splitlines() if line.strip().startswith(prefix)]
    if len(rows) != 1:
        raise ValueError("expected exactly one transaction from the printer")
    return rows[0]

def simulate(tx, url):
    chain = int(tx["chainId"])
    if chain not in (1, 8453, 167000) or int(rpc(url, "eth_chainId", []), 16) != chain:
        raise ValueError("RPC chain does not match prepared transaction")
    for field in ("value", "safeTxGas", "baseGas", "gasPrice"):
        if tx[field] != 0:
            raise ValueError(f"unexpected {field}")
    if tx["gasToken"].lower() != ZERO or tx["refundReceiver"].lower() != ZERO or tx["operation"] not in (0, 1):
        raise ValueError("unexpected transaction fields")
    owners = tx["owners"]
    if tx["threshold"] != 3 or len(owners) != 6 or len({a.lower() for a in owners}) != 6:
        raise ValueError("expected a 3-of-6 Safe")
    if any(not re.fullmatch(r"0x[0-9a-fA-F]{40}", a) for a in owners):
        raise ValueError("invalid owner address")
    block = hex(tx["blockNumber"])
    safe = tx["safe"]
    nonce = int(rpc(url, "eth_call", [{"to": safe, "data": encode("nonce()")}, block]), 16)
    if nonce != tx["nonce"]:
        raise ValueError("Safe nonce differs from prepared transaction")
    hash_data = encode("getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)",
                       tx["to"], 0, tx["data"], tx["operation"], 0, 0, 0, ZERO, ZERO, nonce)
    actual_hash = rpc(url, "eth_call", [{"to": safe, "data": hash_data}, block])
    if actual_hash.lower() != tx["safeTxHash"].lower():
        raise ValueError("Safe transaction hash mismatch")
    # Safe 1.4.x OwnerManager.threshold occupies slot 4; fail closed if that layout differs.
    slot = "0x" + format(4, "064x")
    one = "0x" + format(1, "064x")
    if int(rpc(url, "eth_getStorageAt", [safe, slot, block]), 16) != tx["threshold"]:
        raise ValueError("Safe threshold storage does not match the expected layout")
    sender = min(owners, key=lambda a: int(a, 16))
    signatures = "0x" + sender[2:].lower().zfill(64) + "0" * 64 + "01"
    data = encode(SIGNATURE, tx["to"], 0, tx["data"], tx["operation"], 0, 0, 0, ZERO, ZERO, signatures)
    override = {safe: {"stateDiff": {slot: one}}}
    result = rpc(url, "eth_call", [{"to": safe, "from": sender, "data": data, "gas": hex(8_000_000)}, block, override])
    if int(result, 16) != 1:
        raise ValueError("Safe preflight did not return true")
    row = {"contractAddress": safe, "from": sender, "inputDataType": "raw", "rawFunctionInput": data,
           "gas": "8000000", "stateOverrides": [{"contractAddress": safe, "balance": "", "storage": [{"key": slot, "value": one}]}]}
    payload = {"v": 1, "network": {"id": str(chain)}, "blockNumber": str(tx["blockNumber"]), "row": row}
    def link():
        encoded = base64.urlsafe_b64encode(json.dumps(payload, separators=(",", ":")).encode()).decode().rstrip("=")
        return "https://dashboard.tenderly.co/simulator/new?draft=" + encoded
    url_draft = link()
    embedded = len(url_draft) <= 2000
    if not embedded:
        del row["rawFunctionInput"]
        url_draft = link()
    return {"chainId": chain, "blockNumber": tx["blockNumber"], "safeTxHash": tx["safeTxHash"],
            "preflight": "success", "simulationThreshold": 1, "actualThreshold": tx["threshold"],
            "execTransactionData": data, "tenderlyDraftUrl": url_draft, "calldataEmbedded": embedded}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="command", required=True)
    extract_args = subs.add_parser("extract")
    extract_args.add_argument("--log", type=Path, required=True)
    extract_args.add_argument("--output", type=Path, required=True)
    sim_args = subs.add_parser("simulate")
    sim_args.add_argument("--input", type=Path, required=True)
    sim_args.add_argument("--rpc-url", required=True)
    sim_args.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "extract":
        result = extract(args.log.read_text())
    else:
        result = simulate(json.loads(args.input.read_text()), args.rpc_url)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Saved {args.output}")
    if args.command == "simulate":
        print("Read-only preflight passed; simulation overrides the Safe threshold from 3 to 1.")
        print(result["tenderlyDraftUrl"])
        if not result["calldataEmbedded"]:
            print("Paste execTransactionData from the output file into Tenderly's Raw input data field.")

if __name__ == "__main__":
    main()
