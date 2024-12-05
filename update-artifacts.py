import json
import os
import shutil
import filecmp
import subprocess
import sys
from deepdiff import DeepDiff  # Install with: pip install deepdiff

def compare_field(file1, file2, field) -> bool:
    """Compare a specific field between two JSON files."""
    try:
        # Load JSON files
        with open(file1, 'r') as f1, open(file2, 'r') as f2:
            json1 = json.load(f1)
            json2 = json.load(f2)

        # Extract the specified field
        value1 = json1
        value2 = json2

        # Traverse the JSON to find the specified field
        for key in field.split('.'):
            value1 = value1.get(key) if isinstance(value1, dict) else None
            value2 = value2.get(key) if isinstance(value2, dict) else None

        if value1 is None or value2 is None:
            print(f"Field '{field}' not found in one or both files.")
            return

        # Compare the extracted values
        differences = DeepDiff(value1, value2, verbose_level=2)
        if differences:
            print(f"Differences in field '{field}':")
            print(json.dumps(differences, indent=4))
            return False
        else:
            return True

    except FileNotFoundError as e:
        print(f"Error: {e}")
    except json.JSONDecodeError as e:
        print(f"Error parsing JSON: {e}")
    except Exception as e:
        print(f"Unexpected error: {e}")

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

        if not compare_field(file_path, target_path, "bytecode"):
            print(f"Files differ: {file_path} != {target_path}")
            all_match = False
        else:
            print(f"Files match: {file_path} == {target_path}")

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