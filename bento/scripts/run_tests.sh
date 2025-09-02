#!/bin/bash

# Bento Test Runner Script
# This script runs tests with Docker-based services

set -e  # Exit on any error

echo "🧪 Starting Bento Test Runner..."

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    local color=$1
    local message=$2
    echo -e "${color}${message}${NC}"
}

# Function to check if Docker containers are running
check_docker_services() {
    local postgres_running=false
    local redis_running=false

    if docker ps --format "{{.Names}}" | grep -q "^bento-postgres-test$"; then
        postgres_running=true
    fi

    if docker ps --format "{{.Names}}" | grep -q "^bento-redis-test$"; then
        redis_running=true
    fi

    if [ "$postgres_running" = true ] && [ "$redis_running" = true ]; then
        print_status $GREEN "✅ Docker services are running"
        return 0
    else
        print_status $RED "❌ Docker services are not running"
        echo "   PostgreSQL running: $postgres_running"
        echo "   Redis running: $redis_running"
        echo ""
        print_status $YELLOW "💡 Run './scripts/docker-setup.sh' to start services"
        return 1
    fi
}

# Function to run tests for a specific crate
run_crate_tests() {
    local crate_name=$1
    local test_args=${2:-""}

    echo ""
    print_status $BLUE "🧪 Testing crate: $crate_name"
    echo "   Command: cargo test -p $crate_name $test_args"
    echo "   Environment: DATABASE_URL=$DATABASE_URL"
    echo "   Environment: REDIS_URL=$REDIS_URL"
    echo ""

    if cargo test -p "$crate_name" $test_args; then
        print_status $GREEN "✅ $crate_name tests passed"
        return 0
    else
        print_status $RED "❌ $crate_name tests failed"
        return 1
    fi
}

# Function to run all tests
run_all_tests() {
    echo ""
    print_status $BLUE "🧪 Running all tests..."
    echo "   Command: cargo test --workspace"
    echo "   Environment: DATABASE_URL=$DATABASE_URL"
    echo "   Environment: REDIS_URL=$REDIS_URL"
    echo ""

    if cargo test --workspace; then
        print_status $GREEN "✅ All tests passed"
        return 0
    else
        print_status $RED "❌ Some tests failed"
        return 1
    fi
}

# Function to run specific test types
run_test_types() {
    local test_type=$1

    case $test_type in
        "unit")
            print_status $BLUE "🧪 Running unit tests only..."
            cargo test --workspace --lib
            ;;
        "integration")
            print_status $BLUE "🧪 Running integration tests only..."
            cargo test --workspace --test "*"
            ;;
        "doc")
            print_status $BLUE "🧪 Running documentation tests..."
            cargo test --workspace --doc
            ;;
        "check")
            print_status $BLUE "🔍 Running cargo check..."
            cargo check --workspace
            ;;
        "clippy")
            print_status $BLUE "🔍 Running clippy..."
            cargo clippy --workspace
            ;;
        "fmt")
            print_status $BLUE "🔍 Checking code format..."
            cargo fmt --all -- --check
            ;;
        *)
            print_status $RED "❌ Unknown test type: $test_type"
            echo "   Available types: unit, integration, doc, check, clippy, fmt"
            exit 1
            ;;
    esac
}

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ] || [ ! -d "crates" ]; then
    print_status $RED "❌ Error: This script must be run from the bento directory"
    echo "   Current directory: $(pwd)"
    echo "   Expected: bento directory with Cargo.toml and crates/ subdirectory"
    exit 1
fi

# Check Docker services
if ! check_docker_services; then
    exit 1
fi

# Set environment variables
export DATABASE_URL="postgres://postgres:password@localhost:5432/bento_test"
export REDIS_URL="redis://localhost:6379"
export RISC0_DEV_MODE=true

print_status $BLUE "📋 Environment variables set:"
echo "   DATABASE_URL=$DATABASE_URL"
echo "   REDIS_URL=$REDIS_URL"
echo "   RISC0_DEV_MODE=$RISC0_DEV_MODE"
echo ""

# Parse command line arguments
case "${1:-all}" in
    "all")
        run_all_tests
        ;;
    "unit")
        run_test_types "unit"
        ;;
    "integration")
        run_test_types "integration"
        ;;
    "doc")
        run_test_types "doc"
        ;;
    "check")
        run_test_types "check"
        ;;
    "clippy")
        run_test_types "clippy"
        ;;
    "fmt")
        run_test_types "fmt"
        ;;
    "workflow")
        run_crate_tests "workflow"
        ;;
    "workflow-common")
        run_crate_tests "workflow-common"
        ;;
    "taskdb")
        run_crate_tests "taskdb"
        ;;
    "api")
        run_crate_tests "api"
        ;;
    "bento-client")
        run_crate_tests "bento-client"
        ;;
    "help"|"-h"|"--help")
        echo "Bento Test Runner"
        echo ""
        echo "Usage: $0 [command]"
        echo ""
        echo "Commands:"
        echo "  all              Run all tests (default)"
        echo "  unit             Run unit tests only"
        echo "  integration      Run integration tests only"
        echo "  doc              Run documentation tests"
        echo "  check            Run cargo check"
        echo "  clippy           Run clippy checks"
        echo "  fmt              Check code formatting"
        echo "  workflow         Test workflow crate only"
        echo "  workflow-common  Test workflow-common crate only"
        echo "  taskdb           Test taskdb crate only"
        echo "  api              Test api crate only"
        echo "  bento-client     Test bento-client crate only"
        echo "  help             Show this help message"
        echo ""
        echo "Prerequisites:"
        echo "  - Docker containers must be running"
        echo "  - Run './scripts/docker-setup.sh' to start services"
        echo ""
        echo "Examples:"
        echo "  $0                    # Run all tests"
        echo "  $0 workflow           # Test workflow crate only"
        echo "  $0 check              # Run cargo check"
        exit 0
        ;;
    *)
        print_status $RED "❌ Unknown command: $1"
        echo "   Run '$0 help' for usage information"
        exit 1
        ;;
esac

echo ""
print_status $GREEN "🎉 Test run completed!"
