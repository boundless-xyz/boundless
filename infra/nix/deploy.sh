#!/usr/bin/env bash

# Boundless Infrastructure Deployment Script
# Usage: ./deploy.sh <environment> <instance_type> [hostname]

set -euo pipefail

ENVIRONMENT=${1:-dev}
INSTANCE_TYPE=${2:-manager}
HOSTNAME=${3:-}

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Function to check if hostname is provided
check_hostname() {
    if [ -z "$HOSTNAME" ]; then
        print_error "Hostname is required for deployment"
        echo "Usage: $0 <environment> <instance_type> <hostname>"
        echo "Example: $0 dev manager manager.example.com"
        exit 1
    fi
}

# Function to validate instance type
validate_instance_type() {
    case $INSTANCE_TYPE in
        manager|prover|execution|aux|broker|postgresql|minio)
            print_status "Deploying $INSTANCE_TYPE instance"
            ;;
        *)
            print_error "Invalid instance type: $INSTANCE_TYPE"
            echo "Valid types: manager, prover, execution, aux, broker, postgresql, minio"
            exit 1
            ;;
    esac
}

# Function to build configuration
build_config() {
    print_status "Building NixOS configuration for $INSTANCE_TYPE..."

    if nix build ".#nixosConfigurations.$INSTANCE_TYPE"; then
        print_status "Configuration built successfully"
    else
        print_error "Failed to build configuration"
        exit 1
    fi
}

# Function to deploy configuration
deploy_config() {
    print_status "Deploying to $HOSTNAME..."

    if nixos-rebuild switch --flake ".#$INSTANCE_TYPE" --target-host "$HOSTNAME"; then
        print_status "Deployment successful"
    else
        print_error "Deployment failed"
        exit 1
    fi
}

# Function to verify deployment
verify_deployment() {
    print_status "Verifying deployment..."

    case $INSTANCE_TYPE in
        manager)
            if ssh "$HOSTNAME" "systemctl is-active --quiet boundless-api && systemctl is-active --quiet boundless-broker"; then
                print_status "Manager services are running"
            else
                print_warning "Some manager services may not be running"
            fi
            ;;
        prover)
            if ssh "$HOSTNAME" "systemctl is-active --quiet boundless-prover"; then
                print_status "Prover service is running"
            else
                print_warning "Prover service may not be running"
            fi
            ;;
        execution)
            if ssh "$HOSTNAME" "systemctl is-active --quiet boundless-execution"; then
                print_status "Execution service is running"
            else
                print_warning "Execution service may not be running"
            fi
            ;;
        aux)
            if ssh "$HOSTNAME" "systemctl is-active --quiet boundless-aux"; then
                print_status "Aux service is running"
            else
                print_warning "Aux service may not be running"
            fi
            ;;
        broker)
            if ssh "$HOSTNAME" "systemctl is-active --quiet boundless-broker"; then
                print_status "Broker service is running"
            else
                print_warning "Broker service may not be running"
            fi
            ;;
        postgresql)
            if ssh "$HOSTNAME" "systemctl is-active --quiet postgresql"; then
                print_status "PostgreSQL service is running"
            else
                print_warning "PostgreSQL service may not be running"
            fi
            ;;
        minio)
            if ssh "$HOSTNAME" "systemctl is-active --quiet minio"; then
                print_status "MinIO service is running"
            else
                print_warning "MinIO service may not be running"
            fi
            ;;
    esac
}

# Function to show deployment summary
show_summary() {
    print_status "Deployment Summary:"
    echo "  Environment: $ENVIRONMENT"
    echo "  Instance Type: $INSTANCE_TYPE"
    echo "  Hostname: $HOSTNAME"
    echo "  Status: Deployed successfully"
}

# Main execution
main() {
    print_status "Starting Boundless Infrastructure Deployment"
    print_status "Environment: $ENVIRONMENT"
    print_status "Instance Type: $INSTANCE_TYPE"

    check_hostname
    validate_instance_type
    build_config
    deploy_config
    verify_deployment
    show_summary

    print_status "Deployment completed successfully!"
}

# Run main function
main "$@"
