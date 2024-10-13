#!/usr/bin/env bash

# =============================================================================
# Script Name: boundless_service.sh
# Description:
#   - Manages Docker Compose services with "start" and "stop" commands.
#   - Supports specifying custom environment files.
#
# Usage:
#   ./boundless_service.sh [command] [options]
#
# Commands:
#   start   Start Docker Compose services
#   stop    Stop and remove Docker Compose services
#
# Options:
#   -e, --env-file FILE    Specify a custom environment file (default: ./.env-compose)
#   -h, --help             Display this help message
#
# Examples:
#   ./boundless_service.sh start
#   ./boundless_service.sh start --env-file ./.env.production
#   ./boundless_service.sh stop
#   ./boundless_service.sh stop --env-file ./.env.production
# =============================================================================

# =============================================================================
# Shell Options for Robustness
# =============================================================================

# Exit immediately if a command exits with a non-zero status,
# treat unset variables as an error, and propagate errors in pipelines.
set -euo pipefail

# =============================================================================
# Constants and Defaults
# =============================================================================

DEFAULT_ENV_FILE="./.env-compose"

# =============================================================================
# Helper Functions
# =============================================================================

# Function to display informational messages
log_info() {
    echo -e "\033[34m[INFO]\033[0m $1"
}

# Function to display success messages
log_success() {
    echo -e "\033[32m[SUCCESS]\033[0m $1"
}

# Function to display error messages
log_error() {
    echo -e "\033[31m[ERROR]\033[0m $1" >&2
}

# Function to display usage instructions
usage() {
    cat <<EOF
Usage: $0 [command] [options]

Commands:
  start   Start Docker Compose services
  stop    Stop and remove Docker Compose services

Options:
  -e, --env-file FILE    Specify a custom environment file (default: ./.env-compose)
  -h, --help             Display this help message

Examples:
  $0 start
  $0 start --env-file ./.env.production
  $0 stop
  $0 stop --env-file ./.env.production
EOF
}

# Function to parse command-line arguments
parse_args() {
    # Ensure at least one argument is provided
    if [[ $# -lt 1 ]]; then
        log_error "No command provided."
        usage
        exit 1
    fi

    COMMAND="$1"
    shift

    # Default values
    ENV_FILE="$DEFAULT_ENV_FILE"

    # Parse options
    while [[ $# -gt 0 ]]; do
        case "$1" in
            -e|--env-file)
                if [[ -n "${2-}" && ! "$2" =~ ^- ]]; then
                    ENV_FILE="$2"
                    shift 2
                else
                    log_error "Argument for $1 is missing."
                    usage
                    exit 1
                fi
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                usage
                exit 1
                ;;
        esac
    done

    # Export the environment file variable
    export ENV_FILE
    export COMMAND
}

# Function to verify prerequisites
verify_prerequisites() {
    if ! command -v docker &> /dev/null; then
        log_error "Docker command is not available. Please make sure you have docker in your PATH. (hint: Have you run the setup script?)"
        exit 1
    fi

    if ! docker compose version &> /dev/null; then
        log_error "Docker compose command is not available. Please make sure you have docker in your PATH. (hint: Have you run the setup script?)"
        exit 1
    fi

    if [[ ! -f "$ENV_FILE" ]]; then
        log_error "Environment file '$ENV_FILE' does not exist."
        exit 1
    fi
}

# Function to handle cleanup on interruption (for "start" command)
cleanup() {
    log_info "Interrupt received. Stopping and removing Docker Compose services..."
    docker compose --env-file "$ENV_FILE" down
    log_success "Docker Compose services have been stopped and removed."
    exit 0
}

# =============================================================================
# Command Functions
# =============================================================================

# Function to start Docker Compose services
start_services() {
    # Trap SIGINT (Ctrl-C) and SIGTERM to execute cleanup
    trap cleanup SIGINT SIGTERM

    log_info "Starting Docker Compose services using environment file: $ENV_FILE"

    # Start Docker Compose in foreground mode
    docker compose --profile broker --env-file "$ENV_FILE" up --build -d

    # After docker compose up exits normally (without interruption)
    log_success "Docker Compose services have been started."
}

# Function to stop Docker Compose services
stop_services() {
    log_info "Stopping Docker Compose services using environment file: $ENV_FILE"

    # Stop and remove containers, networks, volumes, and images created by up
    if docker compose --profile broker --env-file "$ENV_FILE" down; then
        log_success "Docker Compose services have been stopped and removed."
    else
        log_error "Failed to stop Docker Compose services."
        exit 1
    fi
}

# =============================================================================
# Main Script Execution
# =============================================================================

# Parse command-line arguments
parse_args "$@"

# Verify prerequisites
verify_prerequisites

# Execute the appropriate command
case "$COMMAND" in
    start)
        start_services
        ;;
    stop)
        stop_services
        ;;
    *)
        log_error "Invalid command: $COMMAND"
        usage
        exit 1
        ;;
esac
