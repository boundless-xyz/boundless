#!/usr/bin/env bash
set -euo pipefail

# Test script for Boundless Nix infrastructure
echo "🧪 Testing Boundless Nix Infrastructure..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test functions
test_flake_syntax() {
    echo -e "${YELLOW}📝 Testing flake syntax...${NC}"
    if nix flake check --extra-experimental-features nix-command --extra-experimental-features flakes; then
        echo -e "${GREEN}✅ Flake syntax is valid${NC}"
    else
        echo -e "${RED}❌ Flake syntax errors found${NC}"
        return 1
    fi
}

test_package_builds() {
    echo -e "${YELLOW}🔨 Testing package builds...${NC}"

    local packages=("manager" "prover" "execution" "aux" "broker")
    local failed=0

    for package in "${packages[@]}"; do
        echo -n "  Building $package... "
        if nix build .#packages.x86_64-linux."$package" --no-link --extra-experimental-features nix-command --extra-experimental-features flakes 2>/dev/null; then
            echo -e "${GREEN}✅${NC}"
        else
            echo -e "${RED}❌${NC}"
            failed=1
        fi
    done

    if [ $failed -eq 0 ]; then
        echo -e "${GREEN}✅ All packages build successfully${NC}"
    else
        echo -e "${RED}❌ Some packages failed to build${NC}"
        return 1
    fi
}

test_configurations() {
    echo -e "${YELLOW}⚙️ Testing NixOS configurations...${NC}"

    local configs=("manager" "prover" "execution" "aux" "broker" "postgresql" "minio")
    local failed=0

    for config in "${configs[@]}"; do
        echo -n "  Testing $config configuration... "
        if nixos-rebuild dry-run --flake .#"$config" --extra-experimental-features nix-command --extra-experimental-features flakes >/dev/null 2>&1; then
            echo -e "${GREEN}✅${NC}"
        else
            echo -e "${RED}❌${NC}"
            failed=1
        fi
    done

    if [ $failed -eq 0 ]; then
        echo -e "${GREEN}✅ All configurations are valid${NC}"
    else
        echo -e "${RED}❌ Some configurations are invalid${NC}"
        return 1
    fi
}

test_secrets() {
    echo -e "${YELLOW}🔐 Testing secrets configuration...${NC}"

    if [ -f "secrets/secrets.yaml" ]; then
        echo -e "${GREEN}✅ Secrets file exists${NC}"

        # Test if secrets can be decrypted
        if sops -d secrets/secrets.yaml >/dev/null 2>&1; then
            echo -e "${GREEN}✅ Secrets can be decrypted${NC}"
        else
            echo -e "${RED}❌ Secrets cannot be decrypted${NC}"
            return 1
        fi
    else
        echo -e "${YELLOW}⚠️ No secrets file found. Run ./setup-secrets.sh first${NC}"
    fi
}

test_environment_variables() {
    echo -e "${YELLOW}🌍 Testing environment variables...${NC}"

    # Test execution environment variables
    local env_vars=$(nix eval .#nixosConfigurations.execution.config.systemd.services.boundless-execution.serviceConfig.Environment --json --extra-experimental-features nix-command --extra-experimental-features flakes)

    if echo "$env_vars" | jq -e '.[] | select(. == "FINALIZE_RETRIES=3")' >/dev/null; then
        echo -e "${GREEN}✅ FINALIZE_RETRIES=3 found${NC}"
    else
        echo -e "${RED}❌ FINALIZE_RETRIES=3 not found${NC}"
        return 1
    fi

    if echo "$env_vars" | jq -e '.[] | select(. == "FINALIZE_TIMEOUT=10")' >/dev/null; then
        echo -e "${GREEN}✅ FINALIZE_TIMEOUT=10 found${NC}"
    else
        echo -e "${RED}❌ FINALIZE_TIMEOUT=10 not found${NC}"
        return 1
    fi
}

test_container() {
    echo -e "${YELLOW}🐳 Testing with NixOS container...${NC}"

    # Check if we can create a container
    if command -v nixos-container >/dev/null 2>&1; then
        echo "  Creating test container..."
        if sudo nixos-container create test-boundless --flake .#manager 2>/dev/null; then
            echo -e "${GREEN}✅ Container created successfully${NC}"

            # Start container
            if sudo nixos-container start test-boundless 2>/dev/null; then
                echo -e "${GREEN}✅ Container started successfully${NC}"

                # Test services
                if sudo nixos-container root-shell test-boundless -c "systemctl is-active boundless-api" 2>/dev/null | grep -q "active"; then
                    echo -e "${GREEN}✅ Services are running${NC}"
                else
                    echo -e "${YELLOW}⚠️ Services not running (this might be expected)${NC}"
                fi

                # Cleanup
                sudo nixos-container stop test-boundless
                sudo nixos-container destroy test-boundless
                echo -e "${GREEN}✅ Container cleaned up${NC}"
            else
                echo -e "${RED}❌ Failed to start container${NC}"
                return 1
            fi
        else
            echo -e "${RED}❌ Failed to create container${NC}"
            return 1
        fi
    else
        echo -e "${YELLOW}⚠️ nixos-container not available, skipping container test${NC}"
    fi
}

# Main test execution
main() {
    local failed=0

    echo "Starting tests..."
    echo "=================="

    test_flake_syntax || failed=1
    echo ""

    test_package_builds || failed=1
    echo ""

    test_configurations || failed=1
    echo ""

    test_secrets || failed=1
    echo ""

    test_environment_variables || failed=1
    echo ""

    test_container || failed=1
    echo ""

    if [ $failed -eq 0 ]; then
        echo -e "${GREEN}🎉 All tests passed!${NC}"
        echo ""
        echo "Next steps:"
        echo "1. Run: ./setup-secrets.sh (if not done already)"
        echo "2. Edit secrets/secrets.yaml with your values"
        echo "3. Deploy: ./deploy.sh <config-name>"
    else
        echo -e "${RED}❌ Some tests failed. Check the output above.${NC}"
        exit 1
    fi
}

# Run tests
main "$@"
