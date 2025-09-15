import tomllib
import re
import sys
from pathlib import Path
from typing import Dict, List


import re
from typing import Dict

def extract_rs_addresses(rs_content: str, network: str) -> Dict[str, str]:
    """
    Parse a block like:

    pub const MAINNET: Deployment = Deployment {
        chain_id: Some(NamedChain::Mainnet as u64),
        boundless_market_address: address!("0x..."),
        verifier_router_address: Some(address!("0x...")),
        set_verifier_address: address!("0x..."),
        collateral_token_address: Some(address!("0x...")),
    };

    Returns a dict mapping *_address field names to lowercase 0x addresses.
    If a field is None or not present, returns '' for that key (or omits it if not present).
    """
    # Grab the struct body for the requested network
    block_pat = rf'pub const {re.escape(network.upper())}\s*:\s*Deployment\s*=\s*Deployment\s*\{{(.*?)\}};'
    m = re.search(block_pat, rs_content, re.DOTALL)
    addresses: Dict[str, str] = {}
    if not m:
        return addresses

    block = m.group(1)

    # Iterate over all "*_address: <value>," lines inside the block
    for m_field in re.finditer(r'(\w+_address)\s*:\s*([^,]+),', block):
        field = m_field.group(1)
        val = m_field.group(2).strip()

        # None -> treat as empty
        if re.fullmatch(r'None', val):
            addresses[field] = ''
            continue

        # Try to extract address inside address!("0x..."), optionally wrapped in Some(...)
        m_addr = re.search(r'address!\(\s*"(?P<addr>0x[a-fA-F0-9]{40})"\s*\)', val)
        if m_addr:
            addresses[field] = m_addr.group('addr').lower()
        else:
            # Fallback: not an address macro (leave empty)
            addresses[field] = ''

    return addresses



def extract_docs_addresses(docs_content: str, network_section: str) -> Dict[str, str]:
    # Capture the section body until the next ### header or end of file
    section_pattern = rf'{re.escape(network_section)}(.*?)(?:\n###|\Z)'
    section_match = re.search(section_pattern, docs_content, re.DOTALL | re.IGNORECASE)
    addresses: Dict[str, str] = {}
    if section_match:
        section = section_match.group(1)

        def grab(label: str) -> str:
            m = re.findall(rf'{label}.*?(0x[a-fA-F0-9]{{40}})', section)
            return (m[0] if m else '').lower()

        addresses['boundless_market_address'] = grab(r'BoundlessMarket')
        addresses['set_verifier_address'] = grab(r'SetVerifier')
        addresses['verifier_router_address'] = grab(r'RiscZeroVerifierRouter')
        addresses['collateral_token_address'] = grab(r'CollateralToken')
        # Some sections may also list these:
        addresses['zkc_address'] = grab(r'\bZKC\b')
        addresses['vezkc_address'] = grab(r'\bveZKC\b')
        addresses['staking_rewards_address'] = grab(r'StakingRewards')
        addresses['povw_accounting_address'] = grab(r'\bPOVW_ACCOUNTING\b')
        addresses['povw_mint_address'] = grab(r'\bPOVW_MINT\b')

    return addresses


def check_todos(docs_content: str) -> List[str]:
    return [line for line in docs_content.splitlines() if 'TODO' in line]


def toml_section(toml_data: dict, network_key: str) -> dict:
    # Networks live under [deployment.<network>]
    return (toml_data.get('deployment') or {}).get(network_key, {}) or {}


def main():
    with open('contracts/deployment.toml', 'rb') as f:
        toml_data = tomllib.load(f)

    rs_content = Path('crates/boundless-market/src/deployments.rs').read_text()
    zkc_rs_content = Path('crates/zkc/src/deployments.rs').read_text()
    povw_rs_content = Path('crates/povw/src/deployments.rs').read_text()
    docs_content = Path('documentation/site/pages/developers/smart-contracts/deployments.mdx').read_text()

    errors = 0

    # Flag any TODOs in docs
    todos = check_todos(docs_content)
    if todos:
        print("❌ Found TODO placeholders in documentation:")
        for todo in todos:
            print("  ", todo)
        errors += len(todos)

    # Docs section headers
    boundless_networks = {
        'base-mainnet': '### Base',
        'base-sepolia': '### Base Sepolia',
        'ethereum-sepolia': '### Ethereum Sepolia'
    }

    zkc_networks = {
        'ethereum-mainnet': '### Ethereum',
        'ethereum-sepolia': '### Ethereum Sepolia',
    }

    # RS const names
    boundless_rs_network_keys = {
        'base-mainnet': 'BASE',
        'base-sepolia': 'BASE_SEPOLIA',
        'ethereum-sepolia': 'SEPOLIA'
    }

    zkc_rs_network_keys = {
        'ethereum-mainnet': 'MAINNET',
        'ethereum-sepolia': 'SEPOLIA',
    }

    # ---- Boundless Market + SetVerifier + Router + CollateralToken ----
    for net_key, docs_header in boundless_networks.items():
        toml_net = toml_section(toml_data, net_key)
        rs_addrs = extract_rs_addresses(rs_content, boundless_rs_network_keys[net_key])
        docs_addrs = extract_docs_addresses(docs_content, docs_header)

        mapping = {
            'boundless-market': 'boundless_market_address',
            'verifier': 'verifier_router_address',
            'set-verifier': 'set_verifier_address',
            'collateral-token': 'collateral_token_address',
        }

        for toml_field, addr_field in mapping.items():
            toml_addr = str(toml_net.get(toml_field, '') or '').lower()
            rs_addr = str(rs_addrs.get(addr_field, '') or '').lower()
            docs_addr = str(docs_addrs.get(addr_field, '') or '').lower()

            # Presence checks (each missing counts as an error)
            if not toml_addr:
                print(f"❌ Missing [deployment.{net_key}] {toml_field} in deployment.toml")
                errors += 1
            if not rs_addr:
                print(f"❌ Missing [{net_key}] {addr_field} in crates/boundless-market/src/deployments.rs")
                errors += 1
            if not docs_addr:
                print(f"❌ Missing [{net_key}] {addr_field} in documentation section '{docs_header}'")
                errors += 1

            # Mismatch checks (only when both sides present)
            if toml_addr and rs_addr and toml_addr != rs_addr:
                print(f"❌ Mismatch [{net_key}] {toml_field} between TOML and RS:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

            if toml_addr and docs_addr and toml_addr != docs_addr:
                print(f"❌ Mismatch [{net_key}] {toml_field} between TOML and documentation:")
                print(f"  TOML: {toml_addr}")
                print(f"  DOCS: {docs_addr}")
                errors += 1

    # ---- ZKC + veZKC (only on Ethereum networks) ----
    for net_key, docs_header in zkc_networks.items():
        toml_net = toml_section(toml_data, net_key)
        rs_addrs = extract_rs_addresses(zkc_rs_content, zkc_rs_network_keys[net_key])

        mapping = {
            'zkc': 'zkc_address',
            'vezkc': 'vezkc_address',
            # TODO: add back once we update the deployment.toml
            # 'zkc-staking-rewards': 'staking_rewards_address',
        }

        for toml_field, addr_field in mapping.items():
            toml_addr = str(toml_net.get(toml_field, '') or '').lower()
            rs_addr = str(rs_addrs.get(addr_field, '') or '').lower()

            # Presence checks
            if not toml_addr:
                print(f"❌ Missing [deployment.{net_key}] {toml_field} in deployment.toml")
                errors += 1
            if not rs_addr:
                print(f"❌ Missing [{net_key}] {addr_field} in crates/zkc/src/deployments.rs")
                errors += 1

            # Mismatches
            if toml_addr and rs_addr and toml_addr != rs_addr:
                print(f"❌ Mismatch [{net_key}] {toml_field} between TOML and RS:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

    # ---- POVW (only on Ethereum networks) ----
    for net_key, docs_header in zkc_networks.items():
        toml_net = toml_section(toml_data, net_key)
        rs_addrs = extract_rs_addresses(povw_rs_content, zkc_rs_network_keys[net_key])

        mapping = {
            'zkc': 'zkc_address',
            'vezkc': 'vezkc_address',
            'povw-accounting': 'povw_accounting_address',
            'povw-mint': 'povw_mint_address',
        }

        for toml_field, addr_field in mapping.items():
            toml_addr = str(toml_net.get(toml_field, '') or '').lower()
            rs_addr = str(rs_addrs.get(addr_field, '') or '').lower()

            # Presence checks
            if not toml_addr:
                print(f"❌ Missing [deployment.{net_key}] {toml_field} in deployment.toml")
                errors += 1
            if not rs_addr:
                print(f"❌ Missing [{net_key}] {addr_field} in crates/povw/src/deployments.rs")
                errors += 1

            # Mismatches
            if toml_addr and rs_addr and toml_addr != rs_addr:
                print(f"❌ Mismatch [{net_key}] {toml_field} between TOML and RS:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

    if errors == 0:
        print("✅ All deployment addresses match across deployment.toml, deployments.rs, and documentation.")
    else:
        print(f"\n❌ Found {errors} issues. Please check inconsistencies or TODO placeholders.")
        sys.exit(1)


if __name__ == '__main__':
    main()
