#!/usr/bin/env python3

import argparse
import os
from pathlib import Path
from tomlkit import parse, dumps

TOML_PATH = Path("contracts/deployment-version-registry.toml")
CHAIN_KEY = os.environ.get("CHAIN_KEY", "anvil")
STACK_TAG = os.environ.get("STACK_TAG", "")

if STACK_TAG:
    DEPLOY_KEY = f"{CHAIN_KEY}-{STACK_TAG}"
else:
    DEPLOY_KEY = CHAIN_KEY

parser = argparse.ArgumentParser(description="Update deployment.<DEPLOY_KEY> fields in version registry TOML file.")

parser.add_argument("--admin", help="VersionRegistry admin address")
parser.add_argument("--version-registry", help="VersionRegistry proxy contract address")
parser.add_argument("--version-registry-impl", help="VersionRegistry impl contract address")
parser.add_argument("--minimum-broker-version", help="Minimum broker version (e.g. '1.2.3')")
parser.add_argument("--notice", help="Governance notice text (empty string to clear)")

args = parser.parse_args()

# Map CLI args to TOML field keys
field_mapping = {
    "admin": args.admin,
    "version-registry": args.version_registry,
    "version-registry-impl": args.version_registry_impl,
    "minimum-broker-version": args.minimum_broker_version,
    "notice": args.notice,
}

# Load TOML file
content = TOML_PATH.read_text()
doc = parse(content)

# Access the relevant section
try:
    section = doc["deployment"][DEPLOY_KEY]
except KeyError:
    raise RuntimeError(f"[deployment.{DEPLOY_KEY}] section not found in {TOML_PATH}")

# Apply updates only for explicitly provided values
for key, value in field_mapping.items():
    if value is not None:
        # Strip whitespace from the value
        if isinstance(value, str):
            value = value.strip()
        section[key] = value
        print(f"Updated '{key}' to '{value}' in [deployment.{DEPLOY_KEY}]")

# Normalize output: no CRLF, strip trailing spaces, final newline
output = dumps(doc)
clean_output = "\n".join(line.rstrip() for line in output.splitlines()) + "\n"
TOML_PATH.write_text(clean_output)

print(f"{TOML_PATH} updated successfully.")
