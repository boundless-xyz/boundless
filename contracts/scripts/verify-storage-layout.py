#!/usr/bin/env python3
"""Assert that the new market and the legacy/ market agree on the storage
slots reachable from both ABIs.

Delegate-calling the legacy impl from the new market's fallback only works if
every (slot, offset, width) that the legacy code touches means the same thing
in the new market's view of storage. This script enforces that by comparing
the storage layouts emitted by `forge build` (extra_output = storageLayout)
for both `contracts/src/BoundlessMarket.sol:BoundlessMarket` and
`contracts/src/legacy/BoundlessMarketLegacy.sol:BoundlessMarket`.

Checks:
  1. Top-level storage variables at slot 0, 1, 2 (requestLocks, accounts,
     imageUrl) agree on label, slot, offset, and normalized type name.
  2. Every struct type that appears in the top-level storage (Account,
     RequestLock, transitively) has identical member layouts in both
     artifacts: same labels, same (slot, offset, width).

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
NEW_ARTIFACT = REPO_ROOT / "out" / "BoundlessMarket.sol" / "BoundlessMarket.json"
LEGACY_ARTIFACT = REPO_ROOT / "out" / "BoundlessMarketLegacy.sol" / "BoundlessMarket.json"

# Top-level storage variables shared between the two contracts. Order +
# expected slot are part of the contract — any change here is a real
# divergence that breaks delegate-call interop.
SHARED_TOP_LEVEL = [
    ("requestLocks", 0),
    ("accounts", 1),
    ("imageUrl", 2),
]


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
        print(f"error: {artifact_path.relative_to(REPO_ROOT)} has no storageLayout (extra_output = storageLayout in foundry.toml?)", file=sys.stderr)
        sys.exit(2)
    return layout


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


def compare_struct_members(name: str, new_members: list, legacy_members: list, errors: list) -> None:
    """Assert two member lists describe the same field at the same (slot, offset)."""
    if len(new_members) != len(legacy_members):
        errors.append(
            f"struct {name}: member count differs (new={len(new_members)}, legacy={len(legacy_members)})"
        )
        return
    for i, (n, l) in enumerate(zip(new_members, legacy_members)):
        for field in ("label", "offset", "slot"):
            if n[field] != l[field]:
                errors.append(
                    f"struct {name} member #{i}: {field} differs (new={n[field]!r}, legacy={l[field]!r})"
                )
        if normalize_type(n["type"]) != normalize_type(l["type"]):
            errors.append(
                f"struct {name} member #{i} ({n['label']}): type differs "
                f"(new={normalize_type(n['type'])!r}, legacy={normalize_type(l['type'])!r})"
            )


def main() -> int:
    new_layout = load_layout(NEW_ARTIFACT)
    legacy_layout = load_layout(LEGACY_ARTIFACT)

    errors: list[str] = []

    # --- Check 1: shared top-level storage variables -----------------------
    for label, slot in SHARED_TOP_LEVEL:
        new_entry = storage_entry(new_layout, slot)
        legacy_entry = storage_entry(legacy_layout, slot)
        if new_entry is None:
            errors.append(f"slot {slot}: missing from new market layout")
            continue
        if legacy_entry is None:
            errors.append(f"slot {slot}: missing from legacy market layout")
            continue
        for field in ("label", "offset", "slot"):
            if new_entry[field] != legacy_entry[field]:
                errors.append(
                    f"slot {slot} ({label}): {field} differs (new={new_entry[field]!r}, legacy={legacy_entry[field]!r})"
                )
        if new_entry["label"] != label:
            errors.append(
                f"slot {slot}: expected label {label!r} in new market, got {new_entry['label']!r}"
            )
        if normalize_type(new_entry["type"]) != normalize_type(legacy_entry["type"]):
            errors.append(
                f"slot {slot} ({label}): type differs after normalization "
                f"(new={normalize_type(new_entry['type'])!r}, legacy={normalize_type(legacy_entry['type'])!r})"
            )

    # --- Check 2: transitively reachable struct layouts --------------------
    labels = [lbl for lbl, _ in SHARED_TOP_LEVEL]
    new_structs = collect_referenced_structs(new_layout, labels)
    legacy_structs = collect_referenced_structs(legacy_layout, labels)

    only_in_new = new_structs - legacy_structs
    only_in_legacy = legacy_structs - new_structs
    if only_in_new:
        errors.append(f"structs referenced by new market but not by legacy: {sorted(only_in_new)}")
    if only_in_legacy:
        errors.append(f"structs referenced by legacy market but not by new: {sorted(only_in_legacy)}")

    new_struct_keys = struct_type_keys(new_layout)
    legacy_struct_keys = struct_type_keys(legacy_layout)

    common = new_structs & legacy_structs
    for normalized_name in sorted(common):
        new_key = new_struct_keys[normalized_name]
        legacy_key = legacy_struct_keys[normalized_name]
        new_members = new_layout["types"][new_key]["members"]
        legacy_members = legacy_layout["types"][legacy_key]["members"]
        compare_struct_members(normalized_name, new_members, legacy_members, errors)

    if errors:
        print("storage layout divergence between src/ and src/legacy/:", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        return 1

    print("OK: storage layout interop preserved between src/ and src/legacy/")
    print(f"    shared top-level slots verified: {len(SHARED_TOP_LEVEL)}")
    print(f"    shared structs verified:         {len(common)} ({', '.join(sorted(common))})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
