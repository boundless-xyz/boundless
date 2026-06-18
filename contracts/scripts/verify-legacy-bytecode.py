#!/usr/bin/env python3
"""Assert that contracts/src/legacy/BoundlessMarketLegacy.sol compiles to
byte-identical bytecode as the OLD BoundlessMarket implementation deployed on
Base mainnet, modulo immutable slots that are baked in at deploy time.

Two checks:
  1. After masking all positions listed in the artifact's
     `immutableReferences`, the legacy artifact and the deployed bytecode are
     byte-identical. This proves the legacy source is the audited code.
  2. The value baked into the deployed bytecode at each known immutable's
     position matches the expected value declared in
     `contracts/test/legacy/deployed-bytecode.meta.toml`. This proves the
     deployment was configured with the verifier / assessor / collateral
     addresses we expect.

If this script fails, do not modify it to make the diff smaller — any drift
between the frozen legacy/ source and the deployed audited bytecode is a real
issue. See contracts/src/legacy/LEGACY-FROZEN.md.

Run via `uv run contracts/scripts/verify-legacy-bytecode.py` from the repo
root, or via `just check-legacy-bytecode`. Requires that `forge build` has
produced the legacy artifact under
`out/BoundlessMarketLegacy.sol/BoundlessMarket.json`.
"""

import json
import os
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
# Defaults check the Cancun/Base legacy. Override via env to check the Taiko legacy compiled under
# FOUNDRY_PROFILE=shanghai:
#   BOUNDLESS_OUT_DIR=out-shanghai
#   BOUNDLESS_LEGACY_SNAPSHOT_DIR=contracts/shanghai/legacy
_OUT_DIR = REPO_ROOT / os.environ.get("BOUNDLESS_OUT_DIR", "out")
_SNAPSHOT_DIR = REPO_ROOT / os.environ.get("BOUNDLESS_LEGACY_SNAPSHOT_DIR", "contracts/test/legacy")
ARTIFACT = _OUT_DIR / "BoundlessMarketLegacy.sol" / "BoundlessMarket.json"
DEPLOYED_HEX = _SNAPSHOT_DIR / "deployed-bytecode.hex"
META_TOML = _SNAPSHOT_DIR / "deployed-bytecode.meta.toml"


def strip0x(s: str) -> str:
    s = s.strip()
    return s[2:] if s.startswith("0x") else s


def parse_immutables_section(text: str) -> dict:
    """Extract the [immutables] table as {name: raw_value_string}.

    Avoids needing tomllib (Python 3.11+) or a third-party TOML parser so this
    works under the system python3 on any CI runner.
    """
    in_section = False
    out = {}
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()  # strip comments + whitespace
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            in_section = line == "[immutables]"
            continue
        if not in_section:
            continue
        m = re.match(r'(\w+)\s*=\s*(.+)$', line)
        if not m:
            raise ValueError(f"unparseable line in [immutables]: {raw!r}")
        key, raw_val = m.group(1), m.group(2).strip()
        if raw_val.startswith('"') and raw_val.endswith('"'):
            raw_val = raw_val[1:-1]
        out[key] = raw_val
    return out


def normalize_to_32_byte_hex(name: str, value: str) -> str:
    """Encode the expected immutable value as a 32-byte lowercase hex string.

    Addresses are 20 bytes left-padded with zeros (right-aligned in the slot).
    bytes32 values are 32 bytes as-is. Integer literals are left-padded.
    """
    if value.startswith("0x"):
        hex_val = value[2:].lower()
        if len(hex_val) == 40:  # address
            return "0" * 24 + hex_val
        if len(hex_val) == 64:  # bytes32
            return hex_val
        raise ValueError(f"{name}: expected 20- or 32-byte hex, got {len(hex_val) // 2} bytes")
    # Integer literal — encode big-endian into 32 bytes.
    try:
        as_int = int(value)
    except ValueError as e:
        raise ValueError(f"{name}: expected hex or integer, got {value!r}") from e
    return f"{as_int:064x}"


def map_immutable_name_to_ast_id(artifact: dict) -> dict:
    """Walk the contract-level immutable declarations in the artifact's AST."""
    result = {}
    for node in artifact["ast"]["nodes"]:
        if node.get("nodeType") != "ContractDefinition":
            continue
        for child in node.get("nodes", []):
            if (
                child.get("nodeType") == "VariableDeclaration"
                and child.get("mutability") == "immutable"
            ):
                result[child["name"]] = str(child["id"])
    return result


def main() -> int:
    if not ARTIFACT.exists():
        print(f"error: {ARTIFACT.relative_to(REPO_ROOT)} not found — run `forge build` first", file=sys.stderr)
        return 2
    if not DEPLOYED_HEX.exists():
        print(f"error: {DEPLOYED_HEX.relative_to(REPO_ROOT)} not found", file=sys.stderr)
        return 2
    if not META_TOML.exists():
        print(f"error: {META_TOML.relative_to(REPO_ROOT)} not found", file=sys.stderr)
        return 2

    artifact = json.loads(ARTIFACT.read_text())
    legacy = strip0x(artifact["deployedBytecode"]["object"]).lower()
    deployed = strip0x(DEPLOYED_HEX.read_text()).lower()

    if len(legacy) != len(deployed):
        print(f"bytecode length differs: legacy={len(legacy)} deployed={len(deployed)} (hex chars)", file=sys.stderr)
        return 1

    # --- Check 1: body matches after masking immutables ---------------------
    refs = artifact["deployedBytecode"].get("immutableReferences", {})
    legacy_arr = list(legacy)
    deployed_arr = list(deployed)
    for occurrences in refs.values():
        for occ in occurrences:
            start_char = occ["start"] * 2
            end_char = start_char + occ["length"] * 2
            for i in range(start_char, end_char):
                legacy_arr[i] = "0"
                deployed_arr[i] = "0"

    if legacy_arr != deployed_arr:
        for i, (a, b) in enumerate(zip(legacy_arr, deployed_arr)):
            if a != b:
                ctx_lo = max(0, i - 20)
                ctx_hi = i + 40
                print("bytecode differs after masking immutables", file=sys.stderr)
                print(f"  first diff at hex char {i} (byte {i // 2})", file=sys.stderr)
                print(f"  legacy:   ...{''.join(legacy_arr[ctx_lo:ctx_hi])}...", file=sys.stderr)
                print(f"  deployed: ...{''.join(deployed_arr[ctx_lo:ctx_hi])}...", file=sys.stderr)
                break
        return 1

    # --- Check 2: expected immutable values match what's baked in -----------
    expected_raw = parse_immutables_section(META_TOML.read_text())
    name_to_id = map_immutable_name_to_ast_id(artifact)

    mismatches = []
    checked = []
    for name, raw_value in expected_raw.items():
        ast_id = name_to_id.get(name)
        if ast_id is None:
            mismatches.append(f"{name}: declared in meta.toml but not found as a contract-level immutable")
            continue
        positions = refs.get(ast_id)
        if not positions:
            mismatches.append(f"{name}: no immutableReferences entry for AST id {ast_id}")
            continue
        try:
            expected_hex = normalize_to_32_byte_hex(name, raw_value)
        except ValueError as e:
            mismatches.append(str(e))
            continue
        # All positions for an immutable carry the same baked value; check the first.
        start = positions[0]["start"] * 2
        actual_hex = deployed[start : start + 64]
        if actual_hex != expected_hex:
            mismatches.append(
                f"{name}: expected {expected_hex} but deployed has {actual_hex} "
                f"(at byte {positions[0]['start']})"
            )
            continue
        checked.append(name)

    # Surface any immutables referenced in the artifact but not declared in meta —
    # likely inherited from a parent contract (e.g. EIP712Upgradeable). Report,
    # don't fail, since those values are derived rather than user-supplied.
    declared_ids = set(name_to_id.values())
    unchecked_inherited = [k for k in refs if k not in declared_ids]

    if mismatches:
        print("immutable value mismatches:", file=sys.stderr)
        for m in mismatches:
            print(f"  - {m}", file=sys.stderr)
        return 1

    bytecode_bytes = len(legacy) // 2
    immutable_positions = sum(len(v) for v in refs.values())
    print(f"OK: legacy/BoundlessMarketLegacy.sol matches deployed OLD impl")
    print(f"    bytecode size: {bytecode_bytes} bytes")
    print(f"    immutable positions masked: {immutable_positions}")
    print(f"    immutable values verified:  {', '.join(checked)}")
    if unchecked_inherited:
        print(
            f"    inherited immutables not in meta (skipped): "
            f"{len(unchecked_inherited)} AST id(s)"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
