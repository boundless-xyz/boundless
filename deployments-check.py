import tomllib
import re
import sys
from pathlib import Path
from typing import Dict, List


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
    If a field is None or not present, returns ''.
    """
    block_pat = rf'pub const {re.escape(network.upper())}\s*:\s*Deployment\s*=\s*Deployment\s*\{{(.*?)\}};'
    m = re.search(block_pat, rs_content, re.DOTALL)
    addresses: Dict[str, str] = {}
    if not m:
        return addresses

    block = m.group(1)

    for m_field in re.finditer(r'(\w+_address)\s*:\s*([^,]+),', block):
        field = m_field.group(1)
        val = m_field.group(2).strip()

        if val == "None":
            addresses[field] = ""
            continue

        m_addr = re.search(r'address!\(\s*"(?P<addr>0x[a-fA-F0-9]{40})"\s*\)', val)
        if m_addr:
            addresses[field] = m_addr.group("addr").lower()
        else:
            addresses[field] = ""

    return addresses


def toml_section(toml_data: dict, network_key: str) -> dict:
    return (toml_data.get("deployment") or {}).get(network_key, {}) or {}


def main():
    with open("contracts/deployment.toml", "rb") as f:
        toml_data = tomllib.load(f)

    rs_content = Path("crates/boundless-market/src/deployments.rs").read_text()
    zkc_rs_content = Path("crates/zkc/src/deployments.rs").read_text()
    povw_rs_content = Path("crates/povw/src/deployments.rs").read_text()

    errors = 0

    # RS const names
    boundless_rs_network_keys = {
        "base-mainnet": "BASE",
        "base-sepolia": "BASE_SEPOLIA",
        "ethereum-sepolia": "SEPOLIA",
    }

    zkc_rs_network_keys = {
        "ethereum-mainnet": "MAINNET",
        "ethereum-sepolia": "SEPOLIA",
    }

    # ---- Boundless Market + SetVerifier + Router + CollateralToken ----
    for net_key in boundless_rs_network_keys:
        toml_net = toml_section(toml_data, net_key)
        rs_addrs = extract_rs_addresses(rs_content, boundless_rs_network_keys[net_key])

        mapping = {
            'boundless-market': 'boundless_market_address',
            'application-verifier': 'verifier_router_address',
            'set-verifier': 'set_verifier_address',
            'collateral-token': 'collateral_token_address',
        }

        for toml_field, addr_field in mapping.items():
            toml_addr = str(toml_net.get(toml_field, "") or "").lower()
            rs_addr = str(rs_addrs.get(addr_field, "") or "").lower()

            # Presence checks
            if not toml_addr:
                print(f"❌ Missing [deployment.{net_key}] {toml_field} in deployment.toml")
                errors += 1
            if not rs_addr:
                print(
                    f"❌ Missing [{net_key}] {addr_field} in crates/boundless-market/src/deployments.rs"
                )
                errors += 1

            if toml_addr and rs_addr and toml_addr != rs_addr:
                print(f"❌ Mismatch [{net_key}] {toml_field} between TOML and RS:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

    # ---- ZKC + veZKC ----
    for net_key in zkc_rs_network_keys:
        toml_net = toml_section(toml_data, net_key)
        rs_addrs = extract_rs_addresses(zkc_rs_content, zkc_rs_network_keys[net_key])

        mapping = {
            "zkc": "zkc_address",
            "vezkc": "vezkc_address",
        }

        for toml_field, addr_field in mapping.items():
            toml_addr = str(toml_net.get(toml_field, "") or "").lower()
            rs_addr = str(rs_addrs.get(addr_field, "") or "").lower()

            if not toml_addr:
                print(f"❌ Missing [deployment.{net_key}] {toml_field} in deployment.toml")
                errors += 1
            if not rs_addr:
                print(
                    f"❌ Missing [{net_key}] {addr_field} in crates/zkc/src/deployments.rs"
                )
                errors += 1

            if toml_addr and rs_addr and toml_addr != rs_addr:
                print(f"❌ Mismatch [{net_key}] {toml_field} between TOML and RS:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

    # ---- POVW ----
    for net_key in zkc_rs_network_keys:
        toml_net = toml_section(toml_data, net_key)
        rs_addrs = extract_rs_addresses(povw_rs_content, zkc_rs_network_keys[net_key])

        mapping = {
            "zkc": "zkc_address",
            "vezkc": "vezkc_address",
            "povw-accounting": "povw_accounting_address",
            "povw-mint": "povw_mint_address",
        }

        for toml_field, addr_field in mapping.items():
            toml_addr = str(toml_net.get(toml_field, "") or "").lower()
            rs_addr = str(rs_addrs.get(addr_field, "") or "").lower()

            if not toml_addr:
                print(f"❌ Missing [deployment.{net_key}] {toml_field} in deployment.toml")
                errors += 1
            if not rs_addr:
                print(
                    f"❌ Missing [{net_key}] {addr_field} in crates/povw/src/deployments.rs"
                )
                errors += 1

            if toml_addr and rs_addr and toml_addr != rs_addr:
                print(f"❌ Mismatch [{net_key}] {toml_field} between TOML and RS:")
                print(f"  TOML: {toml_addr}")
                print(f"  RS  : {rs_addr}")
                errors += 1

    if errors == 0:
        print("✅ All deployment addresses match between deployment.toml and deployments.rs.")
    else:
        print(f"\n❌ Found {errors} issues. Please fix inconsistencies.")
        sys.exit(1)


if __name__ == "__main__":
    main()
