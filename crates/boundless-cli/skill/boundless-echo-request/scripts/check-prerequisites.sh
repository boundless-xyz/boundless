#!/usr/bin/env bash
# Check prerequisites for Boundless echo proof request
set -euo pipefail

# Add common tool install paths
export PATH="$HOME/.cargo/bin:$HOME/.foundry/bin:$HOME/.risc0/bin:$PATH"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

missing=0
warnings=0

check() {
    local name="$1" cmd="$2" install_hint="$3"
    if command -v "$cmd" &>/dev/null; then
        local version
        version=$("$cmd" --version 2>/dev/null | head -1)
        printf "${GREEN}  ✓${NC} %-12s %s\n" "$name" "$version"
    else
        printf "${RED}  ✗${NC} %-12s not found\n" "$name"
        printf "    Install: ${BOLD}%s${NC}\n" "$install_hint"
        ((missing++)) || true
    fi
}

check_optional() {
    local name="$1" cmd="$2" install_hint="$3"
    if command -v "$cmd" &>/dev/null; then
        local version
        version=$("$cmd" --version 2>/dev/null | head -1)
        printf "${GREEN}  ✓${NC} %-12s %s\n" "$name" "$version"
    else
        printf "${YELLOW}  △${NC} %-12s not found (will be installed during setup)\n" "$name"
        printf "    Install: ${BOLD}%s${NC}\n" "$install_hint"
        ((warnings++)) || true
    fi
}

printf "\n"
printf "${BOLD}Boundless Echo Proof — Prerequisite Check${NC}\n"
printf "===========================================\n"
printf "\n"

printf "${BOLD}Required:${NC}\n"
check "Rust"     "rustc"  "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
check "Cargo"    "cargo"  "(installed with Rust)"
check "Foundry"  "cast"   "curl -L https://foundry.paradigm.xyz | bash && foundryup"
check "rzup"     "rzup"   "curl -L https://risczero.com/install | bash"
check "Python 3" "python3" "Install from https://python.org or via your package manager"

printf "\n"
printf "${BOLD}Optional (needed later):${NC}\n"
check_optional "Boundless CLI" "boundless" "cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --branch release-1.2 --bin boundless"

printf "\n"
if [[ $missing -gt 0 ]]; then
    printf "${RED}${BOLD}%d required tool(s) missing.${NC} Install them before continuing.\n" "$missing"
    exit 1
elif [[ $warnings -gt 0 ]]; then
    printf "${GREEN}${BOLD}All required tools found.${NC} ${YELLOW}%d optional tool(s) missing — will install during walkthrough.${NC}\n" "$warnings"
    exit 0
else
    printf "${GREEN}${BOLD}All tools found. Ready to go!${NC}\n"
    exit 0
fi
