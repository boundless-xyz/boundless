#!/usr/bin/env python3
"""
Interactive wizard for deploying Boundless cluster with Ansible.

This script prompts for required configuration values, saves them to a config file,
and can load previously saved configurations.
"""

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Dict, Optional

CONFIG_FILE = Path.home() / ".deploy_cluster_config.json"


def load_config() -> Dict[str, str]:
    """Load configuration from file if it exists."""
    if CONFIG_FILE.exists():
        try:
            with open(CONFIG_FILE, 'r') as f:
                return json.load(f)
        except (json.JSONDecodeError, IOError) as e:
            print(f"Warning: Could not load config file: {e}")
            return {}
    return {}


def save_config(config: Dict[str, str]) -> None:
    """Save configuration to file."""
    try:
        with open(CONFIG_FILE, 'w') as f:
            json.dump(config, f, indent=2)
        print(f"\n✓ Configuration saved to {CONFIG_FILE}")
    except IOError as e:
        print(f"Warning: Could not save config file: {e}")


def prompt_with_default(prompt: str, default: Optional[str] = None,
                        required: bool = True, secret: bool = False) -> str:
    """Prompt user for input with optional default value."""
    if default:
        if secret:
            # For secrets, show masked default instead of actual value
            masked_default = "*" * min(len(default), 20) if default else ""
            prompt_text = f"{prompt} [{masked_default}]: "
        else:
            prompt_text = f"{prompt} [{default}]: "
    else:
        prompt_text = f"{prompt}: "

    if secret:
        import getpass
        value = getpass.getpass(prompt_text).strip()
    else:
        value = input(prompt_text).strip()

    # If user entered nothing, use the default (which is the actual value, not masked)
    if not value:
        if default:
            return default
        elif required:
            print("This field is required. Please enter a value.")
            return prompt_with_default(prompt, default, required, secret)
        else:
            return ""

    return value


def validate_ethereum_address(address: str) -> bool:
    """Basic validation for Ethereum addresses."""
    return address.startswith("0x") and len(address) == 42


def validate_url(url: str) -> bool:
    """Basic validation for URLs."""
    return url.startswith("http://") or url.startswith("https://")


def wizard() -> Dict[str, str]:
    """Run interactive wizard to collect configuration."""
    print("=" * 70)
    print("Boundless Cluster Deployment Wizard")
    print("=" * 70)
    print()

    # Load existing config
    existing_config = load_config()
    use_existing = False

    if existing_config:
        print(f"Found existing configuration in {CONFIG_FILE}")
        response = input("Load existing configuration? [Y/n]: ").strip().lower()
        if response in ('', 'y', 'yes'):
            use_existing = True
            print("\nLoaded existing configuration:")
            for key, value in existing_config.items():
                # Mask sensitive fields
                if any(sensitive in key.upper() for sensitive in ['PASSWORD', 'SECRET', 'PRIVATE_KEY', 'KEY']):
                    print(f"  {key}: {'*' * min(len(str(value)), 20) if value else '(empty)'}")
                else:
                    print(f"  {key}: {value}")
            print()
            response = input("Use this configuration? [Y/n]: ").strip().lower()
            if response in ('', 'y', 'yes'):
                return existing_config

    print("Please provide the following configuration values:")
    print("(Press Enter to use default values or previously saved values)\n")

    config = {}

    # PostgreSQL configuration
    print("--- PostgreSQL Configuration ---")
    config['POSTGRESQL_PASSWORD'] = prompt_with_default(
        "PostgreSQL password",
        existing_config.get('POSTGRESQL_PASSWORD'),
        secret=True
    )

    # MinIO configuration
    print("\n--- MinIO Configuration ---")
    config['MINIO_ROOT_PASSWORD'] = prompt_with_default(
        "MinIO root password",
        existing_config.get('MINIO_ROOT_PASSWORD', 'minioadmin'),
        secret=True
    )
    config['BENTO_S3_BUCKET'] = prompt_with_default(
        "S3 bucket name",
        existing_config.get('BENTO_S3_BUCKET', 'bento')
    )
    config['BENTO_S3_ACCESS_KEY'] = prompt_with_default(
        "S3 access key",
        existing_config.get('BENTO_S3_ACCESS_KEY', 'minioadmin')
    )
    config['BENTO_S3_SECRET_KEY'] = prompt_with_default(
        "S3 secret key",
        existing_config.get('BENTO_S3_SECRET_KEY', 'minioadmin'),
        secret=True
    )
    config['BENTO_S3_URL'] = prompt_with_default(
        "S3 URL",
        existing_config.get('BENTO_S3_URL', 'http://localhost:9000')
    )
    config['BENTO_S3_REGION'] = prompt_with_default(
        "S3 region",
        existing_config.get('BENTO_S3_REGION', 'auto')
    )

    # Broker configuration
    print("\n--- Broker Configuration ---")
    broker_key = prompt_with_default(
        "Broker private key (0x...)",
        existing_config.get('BROKER_PRIVATE_KEY'),
        secret=True
    )
    if broker_key and not validate_ethereum_address(broker_key):
        print("Warning: Private key should start with '0x' and be 42 characters")
    config['BROKER_PRIVATE_KEY'] = broker_key

    config['BROKER_RPC_URL'] = prompt_with_default(
        "Broker RPC URL",
        existing_config.get('BROKER_RPC_URL')
    )
    if config['BROKER_RPC_URL'] and not validate_url(config['BROKER_RPC_URL']):
        print("Warning: RPC URL should start with http:// or https://")

    # Bento configuration
    print("\n--- Bento Configuration ---")
    rewards_address = prompt_with_default(
        "Bento rewards address (0x...)",
        existing_config.get('BENTO_REWARDS_ADDRESS'),
        required=False
    )
    if rewards_address and not validate_ethereum_address(rewards_address):
        print("Warning: Rewards address should start with '0x' and be 42 characters")
    config['BENTO_REWARDS_ADDRESS'] = rewards_address

    povw_log_id = prompt_with_default(
        "Bento POVW log ID (0x...)",
        existing_config.get('BENTO_POVW_LOG_ID'),
        required=False
    )
    if povw_log_id and not validate_ethereum_address(povw_log_id):
        print("Warning: POVW log ID should start with '0x' and be 42 characters")
    config['BENTO_POVW_LOG_ID'] = povw_log_id

    # Bento advanced configuration
    print("\n--- Bento Advanced Configuration ---")
    config['SEGMENT_PO2'] = prompt_with_default(
        "Segment PO2",
        existing_config.get('SEGMENT_PO2', '20')
    )
    config['KECCAK_PO2'] = prompt_with_default(
        "Keccak PO2",
        existing_config.get('KECCAK_PO2', '17')
    )

    # Agent count configuration
    print("\n--- Agent Count Configuration ---")
    print("Specify how many instances of each agent type to deploy:")
    config['BENTO_GPU_COUNT'] = prompt_with_default(
        "Number of GPU (prove) agents",
        existing_config.get('BENTO_GPU_COUNT', '1')
    )
    config['BENTO_EXEC_COUNT'] = prompt_with_default(
        "Number of exec agents",
        existing_config.get('BENTO_EXEC_COUNT', '4')
    )
    config['BENTO_AUX_COUNT'] = prompt_with_default(
        "Number of aux agents",
        existing_config.get('BENTO_AUX_COUNT', '2')
    )

    # Broker configuration
    print("\n--- Broker Configuration (Advanced) ---")
    broker_concurrent_proofs = prompt_with_default(
        "Broker max concurrent proofs",
        existing_config.get('BROKER_MAX_CONCURRENT_PROOFS', '1')
    )
    config['BROKER_MAX_CONCURRENT_PROOFS'] = broker_concurrent_proofs

    # Calculate preflights: exec_count - concurrent_proofs
    try:
        exec_count = int(config['BENTO_EXEC_COUNT'])
        concurrent_proofs = int(broker_concurrent_proofs)
        calculated_preflights = max(1, exec_count - concurrent_proofs)  # Ensure at least 1
        config['BROKER_MAX_CONCURRENT_PREFLIGHTS'] = str(calculated_preflights)
        print(f"  → Calculated max concurrent preflights: {calculated_preflights} (exec_count - concurrent_proofs)")
    except (ValueError, KeyError):
        # Fallback to default if calculation fails
        config['BROKER_MAX_CONCURRENT_PREFLIGHTS'] = existing_config.get('BROKER_MAX_CONCURRENT_PREFLIGHTS', '2')
        print(f"  → Using default max concurrent preflights: {config['BROKER_MAX_CONCURRENT_PREFLIGHTS']}")

    return config


def deploy(config: Dict[str, str], dry_run: bool = False) -> int:
    """Deploy cluster using Ansible with the provided configuration."""
    print("\n" + "=" * 70)
    print("Deploying Boundless Cluster")
    print("=" * 70)
    print()

    # Set environment variables
    env = os.environ.copy()
    extra_vars = {}

    for key, value in config.items():
        if value:  # Only set non-empty values
            # Agent counts and broker config are passed as extra-vars, not env vars
            if key == 'BENTO_GPU_COUNT':
                try:
                    extra_vars['bento_gpu_count'] = int(value)
                except ValueError:
                    print(f"Warning: Invalid number for {key}: {value}, using default")
            elif key == 'BENTO_EXEC_COUNT':
                try:
                    extra_vars['bento_exec_count'] = int(value)
                except ValueError:
                    print(f"Warning: Invalid number for {key}: {value}, using default")
            elif key == 'BENTO_AUX_COUNT':
                try:
                    extra_vars['bento_aux_count'] = int(value)
                except ValueError:
                    print(f"Warning: Invalid number for {key}: {value}, using default")
            elif key == 'BROKER_MAX_CONCURRENT_PROOFS':
                try:
                    extra_vars['broker_max_concurrent_proofs'] = int(value)
                except ValueError:
                    print(f"Warning: Invalid number for {key}: {value}, using default")
            elif key == 'BROKER_MAX_CONCURRENT_PREFLIGHTS':
                try:
                    extra_vars['broker_max_concurrent_preflights'] = int(value)
                except ValueError:
                    print(f"Warning: Invalid number for {key}: {value}, using default")
            else:
                env[key] = value

    # Build ansible-playbook command
    script_dir = Path(__file__).parent.parent
    inventory = script_dir / "inventory.yml"
    playbook = script_dir / "cluster.yml"

    cmd = [
        "ansible-playbook",
        "-i", str(inventory),
        str(playbook),
        "-v"
    ]

    # Add extra-vars for agent counts
    if extra_vars:
        import json
        extra_vars_str = json.dumps(extra_vars)
        cmd.extend(["--extra-vars", extra_vars_str])

    if dry_run:
        cmd.append("--check")
        print("Running in DRY-RUN mode (--check)")

    print(f"Running: {' '.join(cmd)}")
    if extra_vars:
        print(f"Extra vars: {extra_vars}")
    print()

    # Run ansible-playbook
    try:
        result = subprocess.run(cmd, env=env, cwd=str(script_dir))
        return result.returncode
    except FileNotFoundError:
        print("Error: ansible-playbook not found. Is Ansible installed?")
        return 1
    except KeyboardInterrupt:
        print("\n\nDeployment cancelled by user.")
        return 130


def main():
    """Main entry point."""
    import argparse

    parser = argparse.ArgumentParser(
        description="Deploy Boundless cluster with Ansible",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  %(prog)s                    # Interactive wizard
  %(prog)s --load             # Load saved config and deploy
  %(prog)s --check            # Dry-run with wizard
  %(prog)s --load --check     # Dry-run with saved config
        """
    )
    parser.add_argument(
        '--load',
        action='store_true',
        help='Load and use saved configuration without prompting'
    )
    parser.add_argument(
        '--check',
        action='store_true',
        dest='dry_run',
        help='Run in check mode (dry-run, no changes)'
    )
    parser.add_argument(
        '--no-save',
        action='store_true',
        help='Do not save configuration to file'
    )

    args = parser.parse_args()

    # Get configuration
    if args.load:
        config = load_config()
        if not config:
            print(f"Error: No saved configuration found in {CONFIG_FILE}")
            print("Run without --load to create a new configuration.")
            return 1
        print(f"Loaded configuration from {CONFIG_FILE}\n")
    else:
        config = wizard()
        if not args.no_save:
            save_config(config)

    # Confirm before deploying
    if not args.dry_run:
        print("\n" + "=" * 70)
        print("Ready to deploy. This will make changes to the target system(s).")
        response = input("Continue? [y/N]: ").strip().lower()
        if response not in ('y', 'yes'):
            print("Deployment cancelled.")
            return 0

    # Deploy
    return deploy(config, dry_run=args.dry_run)


if __name__ == "__main__":
    sys.exit(main())
