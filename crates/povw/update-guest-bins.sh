#!/bin/bash

# This hacky script will update the built binaries in the elfs folder.
# It should be retired as soon as cargo risczero build has support to more natively handle this e.g. https://github.com/risc0/risc0/pull/3329.

set -eo pipefail

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

cargo risczero build --manifest-path "${SCRIPT_DIR:?}/log-updater/Cargo.toml"
cp "${SCRIPT_DIR:?}/log-updater/target/riscv32im-risc0-zkvm-elf/docker/boundless-povw-log-updater.bin" "${SCRIPT_DIR:?}/elfs"
r0vm --id --elf "${SCRIPT_DIR:?}/elfs/boundless-povw-log-updater.bin" | xxd -r -p > "${SCRIPT_DIR:?}/elfs/boundless-povw-log-updater.iid"

cargo risczero build --manifest-path "${SCRIPT_DIR:?}/mint-calculator/Cargo.toml"
cp "${SCRIPT_DIR:?}/mint-calculator/target/riscv32im-risc0-zkvm-elf/docker/boundless-povw-mint-calculator.bin" "${SCRIPT_DIR:?}/elfs"
r0vm --id --elf "${SCRIPT_DIR:?}/elfs/boundless-povw-mint-calculator.bin" | xxd -r -p > "${SCRIPT_DIR:?}/elfs/boundless-povw-mint-calculator.iid"
