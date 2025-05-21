#!/bin/bash

# =============================================================================
# Script Name: setup.sh
# Description:
#   - Updates the system packages.
#   - Installs essential boundless packages.
#   - Installs GPU drivers for provers.
#   - Installs Docker with NVIDIA support.
#   - Installs Rust programming language.
#   - Installs CUDA Toolkit.
#   - Performs system cleanup.
#   - Verifies Docker with NVIDIA support.
#
# =============================================================================

# Exit immediately if a command exits with a non-zero status,
# treat unset variables as an error, and propagate errors in pipelines.
set -euo pipefail

# =============================================================================
# Constants
# =============================================================================

SCRIPT_NAME="$(basename "$0")"
LOG_FILE="/var/log/${SCRIPT_NAME%.sh}.log"

# =============================================================================
# Functions
# =============================================================================

# Function to display informational messages
info() {
    printf "\e[34m[INFO]\e[0m %s\n" "$1"
}

# Function to display success messages
success() {
    printf "\e[32m[SUCCESS]\e[0m %s\n" "$1"
}

# Function to display error messages
error() {
    printf "\e[31m[ERROR]\e[0m %s\n" "$1" >&2
}

is_package_installed() {
    dpkg -s "$1" &> /dev/null
}


# Function to check if the operating system is Ubuntu
check_os() {
    if [[ -f /etc/os-release ]]; then
        # Source the os-release file to get OS information
        # shellcheck source=/dev/null
        . /etc/os-release
        if [[ "${ID,,}" != "ubuntu" ]]; then
            error "Unsupported operating system: $NAME. This script is intended for Ubuntu."
            exit 1
        elif [[ "${VERSION_ID,,}" != "22.04" && "${VERSION_ID,,}" != "20.04" ]]; then
            error "Unsupported operating system verion: $VERSION. This script is intended for Ubuntu 20.04 or 22.04."
            exit 1
        else
            info "Operating System: $PRETTY_NAME"
        fi
    else
        error "/etc/os-release not found. Unable to determine the operating system."
        exit 1
    fi
}

# Function to update and upgrade the system
update_system() {
    info "Updating and upgrading the system packages..."
    {
        sudo apt update -y
        sudo apt upgrade -y
    } >> "$LOG_FILE" 2>&1
    success "System packages updated and upgraded successfully."
}

# Function to install essential packages
install_packages() {
    local packages=(
        nvtop
        ubuntu-drivers-common
        build-essential
        libssl-dev
        curl
        gnupg
        ca-certificates
        lsb-release
        jq
    )

    info "Installing essential packages: ${packages[*]}..."
    {
        sudo apt install -y "${packages[@]}"
    } >> "$LOG_FILE" 2>&1
    success "Essential packages installed successfully."
}

# Function to install GPU drivers
install_gpu_drivers() {
    info "Detecting and installing appropriate GPU drivers..."
    {
        sudo ubuntu-drivers install
    } >> "$LOG_FILE" 2>&1
    success "GPU drivers installed successfully."
}

# Function to install Rust
install_rust() {
    if command -v rustc &> /dev/null; then
        info "Rust is already installed. Skipping Rust installation."
    else
        info "Installing Rust programming language..."
        {
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        } >> "$LOG_FILE" 2>&1
        # Source Rust environment variables for the current session
        if [[ -f "$HOME/.cargo/env" ]]; then
            # shellcheck source=/dev/null
            source "$HOME/.cargo/env"
            success "Rust installed successfully."
        else
            error "Rust installation failed. ~/.cargo/env not found."
            exit 1
        fi
    fi
}

# Function to install CUDA Toolkit
install_cuda() {
    if is_package_installed "cuda-toolkit"; then
        info "CUDA Toolkit is already installed. Skipping CUDA installation."
    else
        info "Installing CUDA Toolkit and dependencies..."
        {
            local distribution
            distribution=$(grep '^ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"'| tr -d '\.')
            info "Installing Nvidia CUDA keyring and repo"
            wget https://developer.download.nvidia.com/compute/cuda/repos/$distribution/$(/usr/bin/uname -m)/cuda-keyring_1.1-1_all.deb
            sudo dpkg -i cuda-keyring_1.1-1_all.deb
            rm cuda-keyring_1.1-1_all.deb
            sudo apt-get update
            sudo apt-get install -y cuda-toolkit
        } >> "$LOG_FILE" 2>&1
        success "CUDA Toolkit installed successfully."
    fi
}

# Function to install Docker
install_docker() {
    if command -v docker &> /dev/null; then
        info "Docker is already installed. Skipping Docker installation."
    else
        info "Installing Docker..."
        {
            # Install prerequisites
            sudo apt install -y apt-transport-https ca-certificates curl gnupg-agent software-properties-common

            # Add Docker’s official GPG key
            curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg

            # Set up the stable repository
            echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable" | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

            # Update package index
            sudo apt update -y

            # Install Docker Engine, CLI, and Containerd
            sudo apt install -y docker-ce docker-ce-cli containerd.io

            # Enable Docker
            sudo systemctl enable docker

            # Start Docker Service
            sudo systemctl start docker

        } >> "$LOG_FILE" 2>&1
        success "Docker installed and started successfully."
    fi
}

# Function to add user to Docker group
add_user_to_docker_group() {
    local username
    username=$(logname 2>/dev/null || echo "$SUDO_USER")

    if id -nG "$username" | grep -qw "docker"; then
        info "User '$username' is already in the 'docker' group."
    else
        info "Adding user '$username' to the 'docker' group..."
        {
            sudo usermod -aG docker "$username"
        } >> "$LOG_FILE" 2>&1
        success "User '$username' added to the 'docker' group."
        info "To apply the new group membership, please log out and log back in."
    fi
}

# Function to install NVIDIA Container Toolkit
install_nvidia_container_toolkit() {
    info "Checking NVIDIA Container Toolkit installation..."

    if is_package_installed "nvidia-docker2"; then
        success "NVIDIA Container Toolkit (nvidia-docker2) is already installed."
        return
    fi

    info "Installing NVIDIA Container Toolkit..."

    {
        # Add the package repositories
        local distribution
        distribution=$(grep '^ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')
        curl -s -L https://nvidia.github.io/nvidia-docker/gpgkey | sudo apt-key add -
        curl -s -L https://nvidia.github.io/nvidia-docker/"$distribution"/nvidia-docker.list | sudo tee /etc/apt/sources.list.d/nvidia-docker.list

        # Update the package lists
        sudo apt update -y

        # Install the NVIDIA Docker support
        sudo apt install -y nvidia-docker2

        # Restart Docker to apply changes
        sudo systemctl restart docker
    } >> "$LOG_FILE" 2>&1

    success "NVIDIA Container Toolkit installed successfully."
}

# Function to configure Docker daemon for NVIDIA
configure_docker_nvidia() {
    info "Configuring Docker to use NVIDIA runtime by default..."

    {
        # Create Docker daemon configuration directory if it doesn't exist
        sudo mkdir -p /etc/docker

        # Create or overwrite daemon.json with NVIDIA runtime configuration
        sudo tee /etc/docker/daemon.json <<EOF
{
    "default-runtime": "nvidia",
    "runtimes": {
        "nvidia": {
            "path": "nvidia-container-runtime",
            "runtimeArgs": []
        }
    }
}
EOF

        # Restart Docker to apply the new configuration
        sudo systemctl restart docker
    } >> "$LOG_FILE" 2>&1

    success "Docker configured to use NVIDIA runtime by default."
}

# Function to perform system cleanup
cleanup() {
    info "Cleaning up unnecessary packages..."
    {
        sudo apt autoremove -y
        sudo apt autoclean -y
    } >> "$LOG_FILE" 2>&1
    success "Cleanup completed."
}

init_git_submodules() {
    info "ensuring submodules are initialized..."
    {
        git submodule update --init --recursive
    } >> "$LOG_FILE" 2>&1
    success "git submodules initialized successfully"
}

# =============================================================================
# Main Script Execution
# =============================================================================

# Redirect all output to log file
exec > >(tee -a "$LOG_FILE") 2>&1

# Display start message with timestamp
info "===== Script Execution Started at $(date) ====="

# Check if the operating system is Ubuntu
check_os

# ensure all the require source code is present
init_git_submodules

# Update and upgrade the system
update_system

# Install essential packages
install_packages

# Install GPU drivers
install_gpu_drivers

# Install Docker
install_docker

# Add user to Docker group
add_user_to_docker_group

# Install NVIDIA Container Toolkit
install_nvidia_container_toolkit

# Configure Docker to use NVIDIA runtime
configure_docker_nvidia

# Install Rust
install_rust

# Install CUDA Toolkit
install_cuda

# Cleanup
cleanup

success "All tasks completed successfully!"

# Optionally, prompt to reboot if necessary
if [ -t 0 ]; then
    # We're in an interactive terminal
    read -rp "Do you want to reboot now to apply all changes? (y/N): " REBOOT
    case "$REBOOT" in
        [yY][eE][sS]|[yY])
            info "Rebooting the system..."
            reboot
            ;;
        *)
            info "Reboot skipped. Please consider rebooting your system to apply all changes."
            ;;
    esac
else
    # We're in a non-interactive environment (like EC2 user data)
    info "Running in non-interactive mode. Skipping reboot prompt."
fi

# Display end message with timestamp
info "===== Script Execution Ended at $(date) ====="

exit 0
