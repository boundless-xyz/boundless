#!/usr/bin/env python3
"""Assert that BoundlessMarket and every contract delegate-called by it agree
on the storage slots reachable from both ABIs.

Delegate-calling another impl from the new market's `fallback` (legacy) or
from its declared entrypoints (FulfillLib) only works if every (slot, offset,
width) that the peer code touches means the same thing in the new market's
view of storage. This script enforces that by comparing the storage layouts
emitted by `forge build` (extra_output = storageLayout) for:

  - contracts/src/BoundlessMarket.sol           (the new market — primary)
  - contracts/src/legacy/BoundlessMarketLegacy.sol  (fallback target)
  - contracts/src/FulfillLib.sol                (delegate-call target for fulfill)

Checks (per peer):
  1. Top-level storage variables at slot 0, 1, 2 (requestLocks, accounts,
     imageUrl) agree on label, slot, offset, and normalized type name.
  2. Every struct type that appears in the top-level storage (Account,
     RequestLock, transitively) has identical member layouts in both
     artifacts: same labels, same (slot, offset, width).
  3. Shared compile-time constants (SHARED_CONSTANTS, e.g. MARKET_FEE_BPS)
     hold the same declared type and value. Constants occupy no storage slot,
     so this invariant — currently only asserted by a "Must equal
     BoundlessMarket.X" comment in the peer source — is read from the contract
     AST instead of the storage layout. A mismatch means the peer's fee math
     would silently disagree with the market's.

Differences in the AST-id suffix of type names (e.g. `RequestId)12840` vs
`RequestId)20065`) are tolerated — only the human-readable struct/enum/
udvt name matters for delegate-call safety.

Run via `uv run contracts/scripts/verify-storage-layout.py` from the repo
root, or via `just check-storage-layout`. Requires `forge build` to have run.
"""

import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PRIMARY_ARTIFACT = REPO_ROOT / "out" / "BoundlessMarket.sol" / "BoundlessMarket.json"
PEERS = [
    ("legacy", REPO_ROOT / "out" / "BoundlessMarketLegacy.sol" / "BoundlessMarket.json"),
    ("FulfillLib", REPO_ROOT / "out" / "FulfillLib.sol" / "FulfillLib.json"),
]

# Top-level storage variables shared between every contract delegate-called by
# the new market. Order + expected slot are part of the contract — any change
# here is a real divergence that breaks delegate-call interop.
SHARED_TOP_LEVEL = [
    ("requestLocks", 0),
    ("accounts", 1),
    ("imageUrl", 2),
]

# Contract-level `constant`s that the new market and every peer it
# delegate-calls must agree on. These occupy no storage slot, so they are
# checked against the AST rather than the storage layout. Each corresponds to a
# "Must equal BoundlessMarket.X" comment in the peer source — this list turns
# that comment into an enforced invariant.
SHARED_CONSTANTS = ["MARKET_FEE_BPS"]


# Strip the trailing `_storage` suffix and any embedded AST id sequences from
# a type identifier so two artifacts with different compile-time IDs still
# compare equal when their underlying type names match.
#
# Examples:
#   "t_struct(Account)11144_storage" -> "t_struct(Account)_storage"
#   "t_mapping(t_userDefinedValueType(RequestId)12840,t_struct(RequestLock)13116_storage)"
#     -> "t_mapping(t_userDefinedValueType(RequestId),t_struct(RequestLock)_storage)"
_AST_ID_RE = re.compile(r"\)(\d+)")


def normalize_type(t: str) -> str:
    return _AST_ID_RE.sub(")", t)


def load_layout(artifact_path: Path) -> dict:
    if not artifact_path.exists():
        print(f"error: {artifact_path.relative_to(REPO_ROOT)} not found — run `forge build` first", file=sys.stderr)
        sys.exit(2)
    artifact = json.loads(artifact_path.read_text())
    layout = artifact.get("storageLayout")
    if not layout:
        print(
            f"error: {artifact_path.relative_to(REPO_ROOT)} has no storageLayout (extra_output = storageLayout in foundry.toml?)",
            file=sys.stderr,
        )
        sys.exit(2)
    return layout


def load_ast(artifact_path: Path) -> dict:
    if not artifact_path.exists():
        print(f"error: {artifact_path.relative_to(REPO_ROOT)} not found — run `forge build` first", file=sys.stderr)
        sys.exit(2)
    artifact = json.loads(artifact_path.read_text())
    ast = artifact.get("ast")
    if not ast:
        print(
            f"error: {artifact_path.relative_to(REPO_ROOT)} has no ast (ast = true in foundry.toml?)",
            file=sys.stderr,
        )
        sys.exit(2)
    return ast


def _strip_ids(node):
    """Drop source-location / ast-id keys so a value expression compiled in one
    file compares structurally equal to the same expression in another."""
    if isinstance(node, dict):
        return {
            k: _strip_ids(v)
            for k, v in node.items()
            if k not in ("id", "src", "nameLocation", "nameLocations", "referencedDeclaration")
        }
    if isinstance(node, list):
        return [_strip_ids(x) for x in node]
    return node


def find_constant(ast: dict, name: str) -> dict | None:
    """Find a contract-level `constant` declaration by name in the artifact AST.

    Returns its declared type, a structural fingerprint of the value expression
    (robust to literals and to identical multi-token expressions), and a short
    human-readable form. Returns None if no such constant exists.
    """
    for node in ast.get("nodes", []):
        if node.get("nodeType") != "ContractDefinition":
            continue
        for member in node.get("nodes", []):
            if (
                member.get("nodeType") == "VariableDeclaration"
                and member.get("name") == name
                and member.get("constant")
            ):
                value = member.get("value") or {}
                if value.get("nodeType") == "Literal":
                    display = f"{value.get('kind')} {value.get('value')!r}"
                else:
                    display = f"<{value.get('nodeType', 'unknown')} expression>"
                return {
                    "type": member.get("typeDescriptions", {}).get("typeString"),
                    "fingerprint": json.dumps(_strip_ids(value), sort_keys=True),
                    "display": display,
                }
    return None


def compare_constants(primary_ast: dict, peer_ast: dict, peer_name: str) -> list[str]:
    """Assert every SHARED_CONSTANTS entry holds the same declared type and value
    in the primary market and the peer."""
    errors: list[str] = []
    for name in SHARED_CONSTANTS:
        primary = find_constant(primary_ast, name)
        peer = find_constant(peer_ast, name)
        if primary is None:
            errors.append(f"constant {name}: missing from primary (new market)")
            continue
        if peer is None:
            errors.append(f"constant {name}: declared in primary but missing from {peer_name}")
            continue
        if primary["type"] != peer["type"]:
            errors.append(
                f"constant {name}: type differs (primary={primary['type']!r}, {peer_name}={peer['type']!r})"
            )
        if primary["fingerprint"] != peer["fingerprint"]:
            errors.append(
                f"constant {name}: value differs (primary={primary['display']}, {peer_name}={peer['display']})"
            )
    return errors


def storage_entry(layout: dict, slot: int) -> dict | None:
    for entry in layout.get("storage", []):
        if int(entry["slot"]) == slot:
            return entry
    return None


def struct_type_keys(layout: dict) -> dict:
    """Map normalized struct name -> raw type key in the layout's types map.

    Only includes types that have a `members` field (structs).
    """
    out = {}
    for key, val in layout.get("types", {}).items():
        if val.get("members") is None:
            continue
        out[normalize_type(key)] = key
    return out


def collect_referenced_structs(layout: dict, top_level_labels: list[str]) -> set:
    """Walk every type referenced from the named top-level variables and
    return the normalized names of struct types in the transitive closure.

    This is the set of structs whose member layouts must match between
    artifacts for delegate-call interop to be safe.
    """
    types = layout.get("types", {})

    structs = set()
    visited = set()

    def visit(type_key: str) -> None:
        if type_key in visited:
            return
        visited.add(type_key)
        node = types.get(type_key)
        if node is None:
            return
        members = node.get("members")
        if members is not None:
            structs.add(normalize_type(type_key))
            for m in members:
                visit(m["type"])
        # mappings / arrays carry their element type info on the type node itself
        for child_key in ("base", "key", "value"):
            child = node.get(child_key)
            if child is not None:
                visit(child)

    for entry in layout.get("storage", []):
        if entry["label"] in top_level_labels:
            visit(entry["type"])

    return structs


def compare_struct_members(name: str, primary_members: list, peer_members: list, peer_name: str, errors: list) -> None:
    """Assert two member lists describe the same field at the same (slot, offset)."""
    if len(primary_members) != len(peer_members):
        errors.append(
            f"struct {name}: member count differs (primary={len(primary_members)}, {peer_name}={len(peer_members)})"
        )
        return
    for i, (p, q) in enumerate(zip(primary_members, peer_members)):
        for field in ("label", "offset", "slot"):
            if p[field] != q[field]:
                errors.append(
                    f"struct {name} member #{i}: {field} differs (primary={p[field]!r}, {peer_name}={q[field]!r})"
                )
        if normalize_type(p["type"]) != normalize_type(q["type"]):
            errors.append(
                f"struct {name} member #{i} ({p['label']}): type differs "
                f"(primary={normalize_type(p['type'])!r}, {peer_name}={normalize_type(q['type'])!r})"
            )


def compare_layouts(primary_layout: dict, peer_layout: dict, peer_name: str) -> list[str]:
    """Run the storage-layout interop check between the primary (new market)
    and a peer (legacy / FulfillLib). Returns a list of error strings."""

    errors: list[str] = []

    # --- Check 1: shared top-level storage variables -----------------------
    for label, slot in SHARED_TOP_LEVEL:
        primary_entry = storage_entry(primary_layout, slot)
        peer_entry = storage_entry(peer_layout, slot)
        if primary_entry is None:
            errors.append(f"slot {slot}: missing from primary (new market) layout")
            continue
        if peer_entry is None:
            errors.append(f"slot {slot}: missing from {peer_name} layout")
            continue
        for field in ("label", "offset", "slot"):
            if primary_entry[field] != peer_entry[field]:
                errors.append(
                    f"slot {slot} ({label}): {field} differs (primary={primary_entry[field]!r}, {peer_name}={peer_entry[field]!r})"
                )
        if primary_entry["label"] != label:
            errors.append(
                f"slot {slot}: expected label {label!r} in primary, got {primary_entry['label']!r}"
            )
        if normalize_type(primary_entry["type"]) != normalize_type(peer_entry["type"]):
            errors.append(
                f"slot {slot} ({label}): type differs after normalization "
                f"(primary={normalize_type(primary_entry['type'])!r}, {peer_name}={normalize_type(peer_entry['type'])!r})"
            )

    # --- Check 2: transitively reachable struct layouts --------------------
    labels = [lbl for lbl, _ in SHARED_TOP_LEVEL]
    primary_structs = collect_referenced_structs(primary_layout, labels)
    peer_structs = collect_referenced_structs(peer_layout, labels)

    only_in_primary = primary_structs - peer_structs
    only_in_peer = peer_structs - primary_structs
    if only_in_primary:
        errors.append(f"structs referenced by primary but not by {peer_name}: {sorted(only_in_primary)}")
    if only_in_peer:
        errors.append(f"structs referenced by {peer_name} but not by primary: {sorted(only_in_peer)}")

    primary_struct_keys = struct_type_keys(primary_layout)
    peer_struct_keys = struct_type_keys(peer_layout)

    common = primary_structs & peer_structs
    for normalized_name in sorted(common):
        primary_key = primary_struct_keys[normalized_name]
        peer_key = peer_struct_keys[normalized_name]
        primary_members = primary_layout["types"][primary_key]["members"]
        peer_members = peer_layout["types"][peer_key]["members"]
        compare_struct_members(normalized_name, primary_members, peer_members, peer_name, errors)

    return errors


def main() -> int:
    primary_layout = load_layout(PRIMARY_ARTIFACT)
    primary_ast = load_ast(PRIMARY_ARTIFACT)

    any_errors = False
    summary_lines = []
    for peer_name, peer_path in PEERS:
        peer_layout = load_layout(peer_path)
        peer_ast = load_ast(peer_path)
        errors = compare_layouts(primary_layout, peer_layout, peer_name)
        errors += compare_constants(primary_ast, peer_ast, peer_name)
        if errors:
            any_errors = True
            print(f"storage layout divergence between primary (new market) and {peer_name}:", file=sys.stderr)
            for e in errors:
                print(f"  - {e}", file=sys.stderr)
        else:
            labels = [lbl for lbl, _ in SHARED_TOP_LEVEL]
            common = collect_referenced_structs(primary_layout, labels) & collect_referenced_structs(peer_layout, labels)
            summary_lines.append(
                f"  {peer_name:12s} top-level slots: {len(SHARED_TOP_LEVEL)}, shared structs: {len(common)} "
                f"({', '.join(sorted(common))})"
            )

    if any_errors:
        return 1

    print(
        "OK: storage layout interop and shared constants "
        f"({', '.join(SHARED_CONSTANTS)}) preserved across primary, legacy, and FulfillLib"
    )
    for line in summary_lines:
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
