import tomllib
import re
from pathlib import Path

def extract_rs_addresses(rs_content, network):
    pattern = rf'pub const {network.upper()}:.*?\{{(.*?)\}};'
    match = re.search(pattern, rs_content, re.DOTALL)
    addresses = {}
    if match:
        fields = re.findall(r'(\w+_address):\s*"([^"]+)"', match.group(1))
        for field, addr in fields:
            addresses[field] = addr.lower()
    return addresses

def main():
    # Load TOML
    with open("contracts/deployment.toml", "rb") as f:
        toml_data = tomllib.load(f)

    # Load deployments.rs
    rs_content = Path('crates/boundless-market/src/deployments.rs').read_text()

    errors = 0
    networks = {
        'ethereum-sepolia-prod': 'SEPOLIA',
        'base-mainnet': 'BASE'
    }

    for toml_key, rs_key in networks.items():
        toml_section = toml_data.get(toml_key, {})
        rs_addresses = extract_rs_addresses(rs_content, rs_key)

        mapping = {
            'boundless-market': 'boundless_market_address',
            'verifier': 'verifier_router_address',
            'set-verifier': 'set_verifier_address',
            'stake-token': 'stake_token_address'
        }

        for toml_field, rs_field in mapping.items():
            toml_addr = toml_section.get(toml_field, '').lower()
            rs_addr = rs_addresses.get(rs_field, '').lower()
            if toml_addr != rs_addr:
                print(f"❌ Mismatch [{toml_key}] {toml_field}:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

    if errors == 0:
        print("✅ All addresses match between deployment.toml and deployments.rs")
    else:
        print(f"\n❌ Found {errors} address mismatches.")
        exit(1)

if __name__ == '__main__':
    main()
