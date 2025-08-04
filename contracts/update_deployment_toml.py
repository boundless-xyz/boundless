#!/usr/bin/env python3

import argparse
import os
from pathlib import Path
from tomlkit import parse, dumps

TOML_PATH = Path("contracts/deployment.toml")
CHAIN_KEY = os.environ.get("CHAIN_KEY", "anvil")

parser = argparse.ArgumentParser(description="Update deployment.<CHAIN_KEY> fields in TOML file.")

# Deployment fields
parser.add_argument("--admin", help="Admin address")
parser.add_argument("--verifier", help="Verifier contract address")
parser.add_argument("--set-verifier", help="SetVerifier contract address")
parser.add_argument("--boundless-market", help="BoundlessMarket contract address")
parser.add_argument("--boundless-market-impl", help="BoundlessMarket impl contract address")
parser.add_argument("--boundless-market-old-impl", help="BoundlessMarket old impl contract address")
parser.add_argument("--stake-token", help="StakeToken contract address")
parser.add_argument("--assessor-image-id", help="Assessor image ID (hex)")
parser.add_argument("--assessor-guest-url", help="URL to the assessor guest package")

args = parser.parse_args()

# Map CLI args to TOML field keys
field_mapping = {
    "admin": args.admin,
    "verifier": args.verifier,
    "set-verifier": args.set_verifier,
    "boundless-market": args.boundless_market,
    "boundless-market-impl": args.boundless_market_impl,
    "boundless-market-old-impl": args.boundless_market_old_impl,
    "stake-token": args.stake_token,
    "assessor-image-id": args.assessor_image_id,
    "assessor-guest-url": args.assessor_guest_url,
}

# Load TOML file
content = TOML_PATH.read_text()
doc = parse(content)

# Access the relevant section
try:
    section = doc["deployment"][CHAIN_KEY]
except KeyError:
    raise RuntimeError(f"[deployment.{CHAIN_KEY}] section not found in {TOML_PATH}")

# Apply updates only for explicitly provided values
for key, value in field_mapping.items():
    if value is not None:
        section[key] = value
        print(f"Updated '{key}' to '{value}' in [deployment.{CHAIN_KEY}]")

# Normalize output: no CRLF, strip trailing spaces, final newline
output = dumps(doc)
clean_output = "\n".join(line.rstrip() for line in output.splitlines()) + "\n"
TOML_PATH.write_text(clean_output)

print(f"{TOML_PATH} updated successfully.")
