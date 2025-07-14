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


def extract_docs_addresses(docs_content, network_section):
    section_pattern = rf'{network_section}(.*?)(###|$)'
    section_match = re.search(section_pattern, docs_content, re.DOTALL | re.IGNORECASE)
    addresses = {}
    if section_match:
        section = section_match.group(1)
        addresses['boundless_market_address'] = next(iter(re.findall(r'BoundlessMarket.*?(0x[a-fA-F0-9]{40})', section)), '').lower()
        addresses['set_verifier_address'] = next(iter(re.findall(r'SetVerifier.*?(0x[a-fA-F0-9]{40})', section)), '').lower()
        addresses['verifier_router_address'] = next(iter(re.findall(r'RiscZeroVerifierRouter.*?(0x[a-fA-F0-9]{40})', section)), '').lower()
        addresses['stake_token_address'] = next(iter(re.findall(r'StakeToken.*?(0x[a-fA-F0-9]{40})', section)), '').lower()
    return addresses


def check_todos(docs_content):
    todos = [line for line in docs_content.split('\n') if 'TODO' in line]
    return todos


def main():
    with open('contracts/deployment.toml', 'rb') as f:
        toml_data = tomllib.load(f)

    rs_content = Path('crates/boundless-market/src/deployments.rs').read_text()
    docs_content = Path('documentation/site/pages/developers/smart-contracts/deployments.mdx').read_text()

    errors = 0

    todos = check_todos(docs_content)
    if todos:
        print("❌ Found TODO placeholders in documentation:")
        for todo in todos:
            print("  ", todo)
        errors += len(todos)

    networks = {
        'ethereum-sepolia-prod': '### Ethereum Sepolia',
        'base-mainnet': '### Base Mainnet',
        'base-sepolia-prod': '### Base Sepolia'
    }

    rs_network_keys = {
        'ethereum-sepolia-prod': 'SEPOLIA',
        'base-mainnet': 'BASE',
        'base-sepolia-prod': 'BASE_SEPOLIA'
    }

    for toml_key, docs_key in networks.items():
        toml_section = toml_data.get(toml_key, {})
        rs_addresses = extract_rs_addresses(rs_content, rs_network_keys[toml_key])
        docs_addresses = extract_docs_addresses(docs_content, docs_key)

        mapping = {
            'boundless-market': 'boundless_market_address',
            'verifier': 'verifier_router_address',
            'set-verifier': 'set_verifier_address',
            'stake-token': 'stake_token_address',
        }

        for toml_field, addr_field in mapping.items():
            toml_addr = toml_section.get(toml_field, '').lower()
            rs_addr = rs_addresses.get(addr_field, '').lower()
            docs_addr = docs_addresses.get(addr_field, '').lower()

            if toml_addr and rs_addr and toml_addr != rs_addr:
                print(f"❌ Mismatch [{toml_key}] {toml_field} between TOML and RS:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

            if docs_addr and toml_addr and toml_addr != docs_addr:
                print(f"❌ Mismatch [{toml_key}] {toml_field} between TOML and documentation:")
                print(f"  TOML: {toml_addr}")
                print(f"  DOCS: {docs_addr}")
                errors += 1

    if errors == 0:
        print("✅ All deployment addresses match across deployment.toml, deployments.rs, and documentation.")
    else:
        print(f"\n❌ Found {errors} issues. Please check inconsistencies or TODO placeholders.")
        exit(1)


if __name__ == '__main__':
    main()
