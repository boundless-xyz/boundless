#!/usr/bin/env bash
# Check prerequisites for self-hosted Boundless proof request
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
        printf "${GREEN}  ✓${NC} %-16s %s\n" "$name" "$version"
    else
        printf "${RED}  ✗${NC} %-16s not found\n" "$name"
        printf "    Install: ${BOLD}%s${NC}\n" "$install_hint"
        ((missing++)) || true
    fi
}

check_optional() {
    local name="$1" cmd="$2" install_hint="$3"
    if command -v "$cmd" &>/dev/null; then
        local version
        version=$("$cmd" --version 2>/dev/null | head -1)
        printf "${GREEN}  ✓${NC} %-16s %s\n" "$name" "$version"
    else
        printf "${YELLOW}  △${NC} %-16s not found (needed for setup)\n" "$name"
        printf "    Install: ${BOLD}%s${NC}\n" "$install_hint"
        ((warnings++)) || true
    fi
}

printf "\n"
printf "${BOLD}Boundless Self-Hosted Request — Prerequisite Check${NC}\n"
printf "===================================================\n"
printf "\n"

printf "${BOLD}Required:${NC}\n"
check "cloudflared"    "cloudflared"  "brew install cloudflared  OR  https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/"
check "Python 3"       "python3"      "Install from https://python.org or via your package manager"
check "curl"           "curl"         "brew install curl  OR  apt install curl"
check "Boundless CLI"  "boundless"    "cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --branch release-1.2 --bin boundless"
check "Foundry (cast)" "cast"         "curl -L https://foundry.paradigm.xyz | bash && foundryup"

printf "\n"
if [[ $missing -gt 0 ]]; then
    printf "${RED}${BOLD}%d required tool(s) missing.${NC} Install them before continuing.\n" "$missing"
    exit 1
else
    printf "${GREEN}${BOLD}All tools found. Ready to go!${NC}\n"
    exit 0
fi
