import json
import re
import os
import shutil
import subprocess
import sys

def normalize_bytecode(bytecode):
    """
    Normalize bytecode by removing metadata and auxiliary sections.
    """
    if not isinstance(bytecode, str):
        raise ValueError(f"Invalid bytecode: expected string, got {type(bytecode)}")

    # Remove metadata (last 43 bytes of the runtime bytecode if present)
    return re.sub(r"a165627a7a72305820[a-fA-F0-9]{64}0029$", "", bytecode)

def load_bytecode_from_artifact(artifact_path):
    """
    Load bytecode from the artifact JSON file.
    """
    try:
        with open(artifact_path, 'r') as file:
            artifact = json.load(file)

            # Extract the bytecode or deployedBytecode fields
            bytecode = artifact.get("deployedBytecode") or artifact.get("bytecode")
            if not bytecode:
                raise ValueError(f"Bytecode not found in {artifact_path}")

            # Handle cases where the bytecode field might contain additional metadata
            if isinstance(bytecode, dict):
                bytecode = bytecode.get("object")
                if not bytecode:
                    raise ValueError(f"Invalid bytecode format in {artifact_path}")

            return bytecode
    except FileNotFoundError:
        print(f"Error: Artifact file not found: {artifact_path}")
        sys.exit(1)
    except json.JSONDecodeError as e:
        print(f"Error: Failed to parse JSON in {artifact_path}: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"Unexpected error while loading bytecode: {e}")
        sys.exit(1)

def compare_bytecodes(artifact1_path, artifact2_path):
    """
    Compare bytecodes from two artifact JSON files.
    """
    # Load bytecodes from artifacts
    bytecode1 = load_bytecode_from_artifact(artifact1_path)
    bytecode2 = load_bytecode_from_artifact(artifact2_path)

    # Normalize the bytecodes
    normalized1 = normalize_bytecode(bytecode1)
    normalized2 = normalize_bytecode(bytecode2)

    # Compare the normalized bytecodes
    if normalized1 == normalized2:
        return True
    else:
        return False

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

        if not compare_bytecodes(file_path, target_path):
            print(f"NOT functionally equivalent: {file_path} != {target_path}")
            all_match = False
        else:
            print(f"functionally equivalent: {file_path} == {target_path}")

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