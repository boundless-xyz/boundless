#!/usr/bin/env python3
"""Combine verified PoVW and ZKC printer outputs into one read-only Safe preparation."""
import argparse
import copy
import json
from pathlib import Path
import simulate

SAFE = "0x3886eeaf95aa2bdddf0c924925e290291f70447f"
MULTISEND = "0x40a2accbd92bca938b02010e17a5b8929b49130d"
ACCOUNTING = "0x319bd4050b2170a7ae3ead3e6d5ab8a5c7cfbdf8"
MINT = "0xbfce7c2d5e7eddeab71b3eeed770713c8b755397"
ZKC = "0x000006c2a22ff4a44ff1f5d0f2ed65f781f55555"

def upgrade_implementation(data):
    raw = bytes.fromhex(data.removeprefix("0x"))
    if len(raw) != 100 or raw[:4].hex() != "4f1ef286" or any(raw[4:16]) or int.from_bytes(raw[36:68], "big") != 64 or any(raw[68:]):
        raise ValueError("expected upgradeToAndCall(implementation, empty bytes)")
    address = "0x" + raw[16:36].hex()
    if int(address, 16) == 0: raise ValueError("zero implementation")
    return address

def unpack_calls(data):
    raw = bytes.fromhex(data.removeprefix("0x"))
    if raw[:4].hex() != "8d80ff0a" or int.from_bytes(raw[4:36], "big") != 32:
        raise ValueError("expected MultiSend calldata")
    length = int.from_bytes(raw[36:68], "big")
    if len(raw) != 68 + (length + 31) // 32 * 32 or any(raw[68+length:]):
        raise ValueError("invalid MultiSend length or padding")
    packed = raw[68:68+length]
    calls = []
    while packed:
        if len(packed) < 85: raise ValueError("truncated MultiSend call")
        op, to = packed[0], "0x" + packed[1:21].hex()
        value = int.from_bytes(packed[21:53], "big")
        size = int.from_bytes(packed[53:85], "big")
        if op != 0 or value != 0 or len(packed) < 85+size: raise ValueError("invalid upgrade call")
        data = "0x" + packed[85:85+size].hex()
        calls.append((to, data))
        packed = packed[85+size:]
    return calls

def combine(povw, zkc, rpc_url):
    for tx in [povw, zkc]:
        if tx["chainId"] != 1 or tx["safe"].lower() != SAFE: raise ValueError("expected Foundation Safe on Ethereum")
    for key in ["blockNumber", "nonce", "owners", "threshold"]:
        if povw[key] != zkc[key]: raise ValueError("printer outputs must use the same block and Safe state")
    if povw["to"].lower() != MULTISEND or povw["operation"] != 1 or zkc["to"].lower() != ZKC or zkc["operation"] != 0:
        raise ValueError("unexpected source transaction targets")
    calls = unpack_calls(povw["data"])
    if [to for to, _ in calls] != [ACCOUNTING, MINT]: raise ValueError("expected Accounting then Mint")
    calls += [(ZKC, zkc["data"])]
    steps = [{"proxy": to, "implementation": upgrade_implementation(data)} for to, data in calls]
    # Revalidate the two source hashes and preflights at their recorded block.
    simulate.simulate(povw, rpc_url)
    simulate.simulate(zkc, rpc_url)
    packed = b"".join(bytes([0]) + bytes.fromhex(to[2:]) + bytes(32) + len(bytes.fromhex(data[2:])).to_bytes(32, "big") + bytes.fromhex(data[2:]) for to, data in calls)
    fields = ["chainId", "blockNumber", "safe", "to", "value", "operation", "data", "safeTxGas", "baseGas", "gasPrice", "gasToken", "refundReceiver", "nonce", "owners", "threshold", "safeTxHash"]
    tx = {key: copy.deepcopy(povw[key]) for key in fields}
    tx["data"] = simulate.encode("multiSend(bytes)", "0x" + packed.hex())
    hash_data = simulate.encode("getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)", tx["to"], 0, tx["data"], 1, 0, 0, 0, simulate.ZERO, simulate.ZERO, tx["nonce"])
    tx["safeTxHash"] = simulate.rpc(rpc_url, "eth_call", [{"to": tx["safe"], "data": hash_data}, hex(tx["blockNumber"])])
    tx["steps"] = steps
    tx["sourceTransactionHashes"] = [povw["safeTxHash"], zkc["safeTxHash"]]
    return tx

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--povw", type=Path, required=True)
    parser.add_argument("--zkc", type=Path, required=True)
    parser.add_argument("--rpc-url", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    tx = combine(json.loads(args.povw.read_text()), json.loads(args.zkc.read_text()), args.rpc_url)
    result = simulate.simulate(tx, args.rpc_url)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / "transaction.json").write_text(json.dumps(tx, indent=2) + "\n")
    (args.output_dir / "simulation.json").write_text(json.dumps(result, indent=2) + "\n")
    print("Prepared one atomic Accounting, Mint, ZKC Safe transaction. No transaction sent.")
    print(result["tenderlyDraftUrl"])

if __name__ == "__main__": main()
