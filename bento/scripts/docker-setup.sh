#!/bin/bash

# Bento Docker Setup Script
# This script sets up Redis for local testing.

set -e

echo "🐳 Starting Bento Docker Setup..."

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

print_status() {
    local color=$1
    local message=$2
    echo -e "${color}${message}${NC}"
}

check_docker() {
    if ! docker info > /dev/null 2>&1; then
        print_status $RED "❌ Docker is not running or not accessible"
        echo "   Please start Docker Desktop or Docker daemon"
        exit 1
    fi
    print_status $GREEN "✅ Docker is running"
}

container_exists() {
    local container_name=$1
    docker ps -a --format "{{.Names}}" | grep -q "^${container_name}$"
}

cleanup_container() {
    local container_name=$1
    if container_exists "$container_name"; then
        print_status $YELLOW "🧹 Cleaning up existing container: $container_name"
        docker stop "$container_name" > /dev/null 2>&1 || true
        docker rm "$container_name" > /dev/null 2>&1 || true
    fi
}

wait_for_service() {
    local service_name=$1
    local host=$2
    local port=$3
    local max_attempts=30
    local attempt=1

    print_status $BLUE "⏳ Waiting for $service_name to be ready on $host:$port..."

    while [ $attempt -le $max_attempts ]; do
        if nc -z "$host" "$port" 2>/dev/null; then
            print_status $GREEN "✅ $service_name is ready!"
            return 0
        fi

        echo -n "."
        sleep 1
        attempt=$((attempt + 1))
    done

    print_status $RED "❌ $service_name failed to start within $max_attempts seconds"
    return 1
}

check_docker

REDIS_CONTAINER="bento-redis-test"
NETWORK_NAME="bento-test-network"

if ! docker network ls --format "{{.Name}}" | grep -q "^${NETWORK_NAME}$"; then
    print_status $BLUE "🌐 Creating Docker network: $NETWORK_NAME"
    docker network create "$NETWORK_NAME"
else
    print_status $GREEN "✅ Docker network already exists: $NETWORK_NAME"
fi

print_status $BLUE "🔴 Setting up Redis container..."

cleanup_container "$REDIS_CONTAINER"

print_status $BLUE "🚀 Starting Redis container..."
docker run -d \
    --name "$REDIS_CONTAINER" \
    --network "$NETWORK_NAME" \
    -p 6379:6379 \
    redis:latest

if wait_for_service "Redis" "localhost" "6379"; then
    print_status $GREEN "✅ Redis is running and ready"
    print_status $BLUE "   Connection: redis://localhost:6379"
else
    print_status $RED "❌ Failed to start Redis"
    exit 1
fi

export REDIS_URL="redis://localhost:6379"
export RISC0_DEV_MODE=true

print_status $GREEN "🎉 Docker setup completed successfully!"
echo ""
print_status $BLUE "📋 Environment variables set:"
echo "   REDIS_URL=$REDIS_URL"
echo "   RISC0_DEV_MODE=$RISC0_DEV_MODE"
echo ""
print_status $BLUE "💡 Next steps:"
echo "   1. Run tests: ./scripts/run_tests.sh"
echo "   2. Start the API: cargo run -p api -- --redis-url \$REDIS_URL --storage-dir ./data/object_store"
echo "   3. Start agents: cargo run -p workflow -- --task-stream exec --redis-url \$REDIS_URL --storage-dir ./data/object_store --api-url http://localhost:8081"
