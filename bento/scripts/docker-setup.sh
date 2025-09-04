#!/bin/bash

# Bento Docker Setup Script
# This script sets up Docker containers for Redis and PostgreSQL for testing

set -e  # Exit on any error

echo "🐳 Starting Bento Docker Setup..."

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

# Function to check if Docker is running
check_docker() {
    if ! docker info > /dev/null 2>&1; then
        print_status $RED "❌ Docker is not running or not accessible"
        echo "   Please start Docker Desktop or Docker daemon"
        exit 1
    fi
    print_status $GREEN "✅ Docker is running"
}

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

# Function to stop and remove a container
cleanup_container() {
    local container_name=$1
    if container_exists "$container_name"; then
        print_status $YELLOW "🧹 Cleaning up existing container: $container_name"
        docker stop "$container_name" > /dev/null 2>&1 || true
        docker rm "$container_name" > /dev/null 2>&1 || true
    fi
}

# Function to wait for a service to be ready
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

# Check Docker
check_docker

# Container names
POSTGRES_CONTAINER="bento-postgres-test"
REDIS_CONTAINER="bento-redis-test"

# Network name
NETWORK_NAME="bento-test-network"

# Create network if it doesn't exist
if ! docker network ls --format "{{.Name}}" | grep -q "^${NETWORK_NAME}$"; then
    print_status $BLUE "🌐 Creating Docker network: $NETWORK_NAME"
    docker network create "$NETWORK_NAME"
else
    print_status $GREEN "✅ Docker network already exists: $NETWORK_NAME"
fi

# Setup PostgreSQL
print_status $BLUE "🐘 Setting up PostgreSQL container..."

cleanup_container "$POSTGRES_CONTAINER"

print_status $BLUE "🚀 Starting PostgreSQL container..."
docker run -d \
    --name "$POSTGRES_CONTAINER" \
    --network "$NETWORK_NAME" \
    -e POSTGRES_PASSWORD=password \
    -e POSTGRES_DB=bento_test \
    -e POSTGRES_USER=postgres \
    -p 5432:5432 \
    postgres:latest

# Wait for PostgreSQL to be ready
if wait_for_service "PostgreSQL" "localhost" "5432"; then
    print_status $GREEN "✅ PostgreSQL is running and ready"
    print_status $BLUE "   Connection: postgres://postgres:password@localhost:5432/bento_test"
else
    print_status $RED "❌ Failed to start PostgreSQL"
    exit 1
fi

# Setup Redis
print_status $BLUE "🔴 Setting up Redis container..."

cleanup_container "$REDIS_CONTAINER"

print_status $BLUE "🚀 Starting Redis container..."
docker run -d \
    --name "$REDIS_CONTAINER" \
    --network "$NETWORK_NAME" \
    -p 6379:6379 \
    redis:latest

# Wait for Redis to be ready
if wait_for_service "Redis" "localhost" "6379"; then
    print_status $GREEN "✅ Redis is running and ready"
    print_status $BLUE "   Connection: redis://localhost:6379"
else
    print_status $RED "❌ Failed to start Redis"
    exit 1
fi

# Set environment variables
export DATABASE_URL="postgres://postgres:password@localhost:5432/bento_test"
export REDIS_URL="redis://localhost:6379"

# Run database migrations
print_status $BLUE "🗄️  Running database migrations..."

# Wait a bit more for PostgreSQL to be fully ready
print_status $BLUE "⏳ Waiting for PostgreSQL to be fully ready for migrations..."
sleep 5

# Function to test database connection
test_db_connection() {
    local max_attempts=15
    local attempt=1

    while [ $attempt -le $max_attempts ]; do
        # Try to connect using psql or a simple TCP connection test
        if docker exec bento-postgres-test pg_isready -U postgres > /dev/null 2>&1; then
            return 0
        fi

        print_status $YELLOW "⏳ Database not ready for migrations, attempt $attempt/$max_attempts..."
        sleep 3
        attempt=$((attempt + 1))
    done

    return 1
}

# Test database connection before running migrations
if test_db_connection; then
    print_status $GREEN "✅ Database connection verified"
else
    print_status $RED "❌ Database connection failed after multiple attempts"
    exit 1
fi

# Check if sqlx CLI is available
if ! command -v sqlx &> /dev/null; then
    print_status $YELLOW "⚠️  sqlx CLI not found, installing..."
    cargo install sqlx-cli --no-default-features --features postgres
fi

# Run migrations for each crate that has them
print_status $BLUE "🔄 Discovering and running migrations..."

# Function to run migrations for a crate
run_crate_migrations() {
    local crate_name=$1
    local crate_path="crates/$crate_name"

    if [ -d "$crate_path/migrations" ]; then
        print_status $BLUE "🔄 Running migrations for $crate_name..."
        cd "$crate_path"

                # Create database if it doesn't exist (with retry)
        local db_attempts=0
        local max_db_attempts=5

        while [ $db_attempts -lt $max_db_attempts ]; do
            if sqlx database create --database-url "$DATABASE_URL" 2>/dev/null; then
                print_status $BLUE "✅ Database created for $crate_name"
                break
            else
                # Check if database already exists
                if sqlx database drop --database-url "$DATABASE_URL" --force 2>/dev/null; then
                    print_status $BLUE "🔄 Recreating database for $crate_name"
                    if sqlx database create --database-url "$DATABASE_URL"; then
                        print_status $BLUE "✅ Database recreated for $crate_name"
                        break
                    fi
                fi

                db_attempts=$((db_attempts + 1))
                if [ $db_attempts -lt $max_db_attempts ]; then
                    print_status $YELLOW "⏳ Database creation failed, retrying in 2 seconds... (attempt $db_attempts/$max_db_attempts)"
                    sleep 2
                else
                    print_status $RED "❌ Failed to create database for $crate_name after $max_db_attempts attempts"
                    cd ../..
                    return 1
                fi
            fi
        done

        # Run migrations
        if sqlx migrate run --database-url "$DATABASE_URL"; then
            print_status $GREEN "✅ $crate_name migrations completed"
        else
            print_status $RED "❌ $crate_name migrations failed"
            cd ../..
            return 1
        fi

        cd ../..
        return 0
    else
        print_status $YELLOW "⚠️  No migrations found for $crate_name"
        return 0
    fi
}

# List of crates that might have migrations
CRATES_WITH_MIGRATIONS=("taskdb" "broker" "indexer" "order-stream" "slasher")

# Run migrations for each crate
for crate in "${CRATES_WITH_MIGRATIONS[@]}"; do
    if ! run_crate_migrations "$crate"; then
        print_status $RED "❌ Migration process failed for $crate"
        exit 1
    fi
done

print_status $GREEN "🎉 Docker setup completed successfully!"
echo ""
print_status $BLUE "📋 Environment variables set:"
echo "   DATABASE_URL=$DATABASE_URL"
echo "   REDIS_URL=$REDIS_URL"
echo ""
print_status $BLUE "🐳 Running containers:"
docker ps --filter "name=bento-*" --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}"
echo ""
print_status $BLUE "💡 Next steps:"
echo "   1. Run tests: ./scripts/run_tests.sh"
echo "   2. Or run basic tests: ./scripts/run_basic_tests.sh"
echo "   3. Stop containers: ./scripts/docker-cleanup.sh"
echo ""
print_status $YELLOW "⚠️  Note: These containers will persist until manually stopped"
print_status $YELLOW "   Use './docker-cleanup.sh' to stop and remove them"
