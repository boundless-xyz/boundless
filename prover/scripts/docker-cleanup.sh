#!/bin/bash

# Bento Docker Cleanup Script
# This script stops and removes Docker containers and networks used for testing

set -e  # Exit on any error

echo "🧹 Starting Bento Docker Cleanup..."

# Colors for output
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

# Container name
REDIS_CONTAINER="bento-redis-test"

# Network name
NETWORK_NAME="bento-test-network"

# Function to check if a container exists
container_exists() {
    local container_name=$1
    docker ps -a --format "{{.Names}}" | grep -q "^${container_name}$"
}

# Function to check if a container is running
container_running() {
    local container_name=$1
    docker ps --format "{{.Names}}" | grep -q "^${container_name}$"
}

# Function to check if a network exists
network_exists() {
    docker network ls --format "{{.Name}}" | grep -q "^${NETWORK_NAME}$"
}

# Stop and remove containers
print_status $BLUE "🛑 Stopping and removing containers..."

if container_exists "$REDIS_CONTAINER"; then
    if container_running "$REDIS_CONTAINER"; then
        print_status $YELLOW "⏹️  Stopping Redis container..."
        docker stop "$REDIS_CONTAINER"
    fi
    print_status $YELLOW "🗑️  Removing Redis container..."
    docker rm "$REDIS_CONTAINER"
    print_status $GREEN "✅ Redis container cleaned up"
else
    print_status $GREEN "✅ Redis container not found"
fi

# Remove network
print_status $BLUE "🌐 Removing Docker network..."

if network_exists; then
    print_status $YELLOW "🗑️  Removing network: $NETWORK_NAME"
    docker network rm "$NETWORK_NAME"
    print_status $GREEN "✅ Network removed"
else
    print_status $GREEN "✅ Network not found"
fi

# Clean up any dangling images (optional)
read -p "🧹 Remove dangling Docker images? (y/N): " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    print_status $BLUE "🧹 Removing dangling images..."
    docker image prune -f
    print_status $GREEN "✅ Dangling images removed"
fi

# Clean up any dangling volumes (optional)
read -p "🧹 Remove dangling Docker volumes? (y/N): " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    print_status $BLUE "🧹 Removing dangling volumes..."
    docker volume prune -f
    print_status $GREEN "✅ Dangling volumes removed"
fi

print_status $GREEN "🎉 Docker cleanup completed successfully!"
echo ""
print_status $BLUE "📋 Summary:"
echo "   ✅ Containers stopped and removed"
echo "   ✅ Network removed"
echo "   ✅ Environment cleaned up"
echo ""
print_status $BLUE "💡 To start fresh:"
echo "   ./scripts/docker-setup.sh"
