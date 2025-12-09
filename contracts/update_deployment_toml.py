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
parser.add_argument("--admin-2", help="Secondary admin address")
parser.add_argument("--verifier", help="Verifier contract address")
parser.add_argument("--application-verifier", help="Application Verifier contract address")
parser.add_argument("--set-verifier", help="SetVerifier contract address")
parser.add_argument("--boundless-market", help="BoundlessMarket contract address")
parser.add_argument("--boundless-market-impl", help="BoundlessMarket impl contract address")
parser.add_argument("--boundless-market-old-impl", help="BoundlessMarket old impl contract address")
parser.add_argument("--collateral-token", help="CollateralToken contract address")
parser.add_argument("--assessor-image-id", help="Assessor image ID (hex)")
parser.add_argument("--assessor-guest-url", help="URL to the assessor guest package")

# PoVW contract fields
parser.add_argument("--povw-accounting", help="PovwAccounting contract address")
parser.add_argument("--povw-accounting-impl", help="PovwAccounting impl contract address")
parser.add_argument("--povw-accounting-old-impl", help="PovwAccounting old impl contract address")
parser.add_argument("--povw-accounting-admin", help="PovwAccounting admin address")
parser.add_argument("--povw-mint", help="PovwMint contract address")
parser.add_argument("--povw-mint-impl", help="PovwMint impl contract address")
parser.add_argument("--povw-mint-old-impl", help="PovwMint old impl contract address")
parser.add_argument("--povw-mint-admin", help="PovwMint admin address")

# PoVW image ID fields
parser.add_argument("--povw-log-updater-id", help="PoVW log updater image ID (hex)")
parser.add_argument("--povw-mint-calculator-id", help="PoVW mint calculator image ID (hex)")

# Deployment commit fields
parser.add_argument("--boundless-market-deployment-commit", help="Boundless Market deployment commit hash")
parser.add_argument("--povw-accounting-deployment-commit", help="PoVW accounting deployment commit hash")
parser.add_argument("--povw-mint-deployment-commit", help="PoVW mint deployment commit hash")

# ZKC contract fields
parser.add_argument("--zkc", help="ZKC contract address")
parser.add_argument("--vezkc", help="veZKC contract address")

args = parser.parse_args()

# Map CLI args to TOML field keys
field_mapping = {
    "admin": args.admin,
    "admin-2": getattr(args, 'admin_2'),
    "verifier": args.verifier,
    "application-verifier": args.application_verifier,
    "set-verifier": args.set_verifier,
    "boundless-market": args.boundless_market,
    "boundless-market-impl": args.boundless_market_impl,
    "boundless-market-old-impl": args.boundless_market_old_impl,
    "collateral-token": args.collateral_token,
    "assessor-image-id": args.assessor_image_id,
    "assessor-guest-url": args.assessor_guest_url,
    # PoVW contract fields
    "povw-accounting": args.povw_accounting,
    "povw-accounting-impl": args.povw_accounting_impl,
    "povw-accounting-old-impl": args.povw_accounting_old_impl,
    "povw-accounting-admin": args.povw_accounting_admin,
    "povw-mint": args.povw_mint,
    "povw-mint-impl": args.povw_mint_impl,
    "povw-mint-old-impl": args.povw_mint_old_impl,
    "povw-mint-admin": args.povw_mint_admin,
    # PoVW image ID fields
    "povw-log-updater-id": args.povw_log_updater_id,
    "povw-mint-calculator-id": args.povw_mint_calculator_id,
    # Deployment commit fields
    "boundless-market-deployment-commit": args.boundless_market_deployment_commit,
    "povw-accounting-deployment-commit": args.povw_accounting_deployment_commit,
    "povw-mint-deployment-commit": args.povw_mint_deployment_commit,
    # ZKC contract fields
    "zkc": args.zkc,
    "vezkc": args.vezkc,
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
        # Strip whitespace from the value
        if isinstance(value, str):
            value = value.strip()
        section[key] = value
        print(f"Updated '{key}' to '{value}' in [deployment.{CHAIN_KEY}]")

# Normalize output: no CRLF, strip trailing spaces, final newline
output = dumps(doc)
clean_output = "\n".join(line.rstrip() for line in output.splitlines()) + "\n"
TOML_PATH.write_text(clean_output)

print(f"{TOML_PATH} updated successfully.")