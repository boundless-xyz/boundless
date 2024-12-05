import json
import re
import os
import shutil
import subprocess
import sys

def get_bytecode(artifact):
    """Extract bytecode from Forge artifact structure"""
    return artifact.get('bytecode', {}).get('object', '')

def normalize_bytecode(bytecode: str) -> str:
    """Remove metadata and constructor data"""
    bytecode = bytecode.replace('0x', '')
    bytecode = re.sub(r'64697066735822[0-9a-f]+64736f6c6343[0-9a-f]+0033$', '', bytecode)
    bytecode = re.sub(r'a165627a7a72305820[0-9a-f]{64}0029$', '', bytecode)
    bytecode = re.sub(r'0033$', '', bytecode)
    return bytecode

def compare_contracts(path1, path2) -> bool:
    with open(path1) as f1, open(path2) as f2:
        artifact1 = json.load(f1)
        artifact2 = json.load(f2)

    bytecode1 = get_bytecode(artifact1)
    bytecode2 = get_bytecode(artifact2)
    
    norm1 = normalize_bytecode(bytecode1)
    norm2 = normalize_bytecode(bytecode2)

    if norm1 != norm2:
        return False
    return True

def run_forge_build():
    """Run `forge build` command."""
    try:
        print("Running `forge build`...")
        result = subprocess.run(["forge", "build"], check=True, capture_output=True, text=True)
        print(result.stdout)
        print("`forge build` completed successfully.")
    except subprocess.CalledProcessError as e:
        print("Error during `forge build`:")
        print(e.stderr)
        raise

def copy_files(file_list, target_folder):
    os.makedirs(target_folder, exist_ok=True)
    
    for file_path in file_list:
        if not os.path.exists(file_path):
            print(f"Warning: {file_path} does not exist. Skipping.")
            continue

        try:
            file_name = os.path.basename(file_path)
            target_path = os.path.join(target_folder, file_name)
            shutil.copy2(file_path, target_path)
            print(f"Copied: {file_path} -> {target_path}")
        except Exception as e:
            print(f"Error copying {file_path} to {target_folder}: {e}")

def check_bytecode_diffs(file_list, target_folder):
    """Check if the differences between files in file_list and target_folder are empty."""
    all_match = True

    for file_path in file_list:
        file_name = os.path.basename(file_path)
        target_path = os.path.join(target_folder, file_name)

        if not os.path.exists(file_path):
            print(f"Source file does not exist: {file_path}")
            all_match = False
            continue

        if not os.path.exists(target_path):
            print(f"Target file does not exist: {target_path}")
            all_match = False
            continue

        if not compare_contracts(file_path, target_path):
            print(f"NOT functionally equivalent: {file_path} != {target_path}")
            all_match = False
        else:
            print(f"Functionally equivalent: {file_path} == {target_path}")

    if not all_match:
        raise RuntimeError("Differences detected between compiled contracts and artifacts.")


file_list = [
    "contracts/out/RiscZeroMockVerifier.sol/RiscZeroMockVerifier.json",
    "contracts/out/RiscZeroSetVerifier.sol/RiscZeroSetVerifier.json",
    "contracts/out/BoundlessMarket.sol/BoundlessMarket.json",
    "contracts/out/ERC1967Proxy.sol/ERC1967Proxy.json"
]

target_folder = "crates/boundless-market/src/contracts/artifacts"

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python script.py <command>")
        print("Commands:")
        print("  copy   - Run forge build and copy contracts to artifact folder")
        print("  check  - Check differences between compiled contracts and artifacts")
        sys.exit(1)

    command = sys.argv[1]

    try:
        if command == "copy":
            run_forge_build()
            copy_files(file_list, target_folder)
        elif command == "check":
            run_forge_build()
            check_bytecode_diffs(file_list, target_folder)
            print("All files match.")
        else:
            print(f"Unknown command: {command}")
            sys.exit(1)
    except RuntimeError as e:
        print(f"Error: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"Unexpected error: {e}")
        sys.exit(1)