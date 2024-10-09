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

# Function to check if the script is run as root
check_root() {
    if [[ "$EUID" -ne 0 ]]; then
        error "This script must be run with sudo or as root."
        exit 1
    fi
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
        apt update -y
        apt upgrade -y
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
    )

    info "Installing essential packages: ${packages[*]}..."
    {
        apt install -y "${packages[@]}"
    } >> "$LOG_FILE" 2>&1
    success "Essential packages installed successfully."
}

# Function to install GPU drivers
install_gpu_drivers() {
    info "Detecting and installing appropriate GPU drivers..."
    {
        ubuntu-drivers install
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
    if dpkg -l | grep -q "^ii  cuda-toolkit"; then
        info "CUDA Toolkit is already installed. Skipping CUDA installation."
    else
        info "Installing CUDA Toolkit and dependencies..."
        {
            local distribution
            distribution=$(grep '^ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"'| tr -d '\.')
            info "Installing Nvidia CUDA keyring and repo"
            wget https://developer.download.nvidia.com/compute/cuda/repos/$distribution/$(/usr/bin/uname -m)/cuda-keyring_1.1-1_all.deb
            dpkg -i cuda-keyring_1.1-1_all.deb
            rm cuda-keyring_1.1-1_all.deb
            apt-get update
            apt-get install -y cuda-toolkit
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
            apt install -y apt-transport-https ca-certificates curl gnupg-agent software-properties-common

            # Add Docker’s official GPG key
            curl -fsSL https://download.docker.com/linux/ubuntu/gpg | gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg

            # Set up the stable repository
            echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable" | tee /etc/apt/sources.list.d/docker.list > /dev/null

            # Update package index
            apt update -y

            # Install Docker Engine, CLI, and Containerd
            apt install -y docker-ce docker-ce-cli containerd.io

            # Enable Docker
            systemctl enable docker

            # Start Docker Service
            systemctl start docker

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
            usermod -aG docker "$username"
        } >> "$LOG_FILE" 2>&1
        success "User '$username' added to the 'docker' group."
        info "To apply the new group membership, please log out and log back in."
    fi
}

# Function to install NVIDIA Container Toolkit
install_nvidia_container_toolkit() {
    info "Installing NVIDIA Container Toolkit..."

    {
        # Add the package repositories
        local distribution
        distribution=$(grep '^ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')
        curl -s -L https://nvidia.github.io/nvidia-docker/gpgkey | apt-key add -
        curl -s -L https://nvidia.github.io/nvidia-docker/"$distribution"/nvidia-docker.list | tee /etc/apt/sources.list.d/nvidia-docker.list

        # Update the package lists
        apt update -y

        # Install the NVIDIA Docker support
        apt install -y nvidia-docker2

        # Restart Docker to apply changes
        systemctl restart docker
    } >> "$LOG_FILE" 2>&1

    success "NVIDIA Container Toolkit installed successfully."
}

# Function to configure Docker daemon for NVIDIA
configure_docker_nvidia() {
    info "Configuring Docker to use NVIDIA runtime by default..."

    {
        # Create Docker daemon configuration directory if it doesn't exist
        mkdir -p /etc/docker

        # Create or overwrite daemon.json with NVIDIA runtime configuration
        cat > /etc/docker/daemon.json <<EOF
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
        systemctl restart docker
    } >> "$LOG_FILE" 2>&1

    success "Docker configured to use NVIDIA runtime by default."
}

# Function to verify Docker with NVIDIA support
verify_docker_nvidia() {
    info "Verifying Docker and NVIDIA setup..."

    if docker run --rm --gpus all nvidia/cuda:12.2.0-devel-ubuntu22.04 nvidia-smi >> "$LOG_FILE" 2>&1; then
        success "Docker with NVIDIA support is working correctly."
    else
        error "Docker with NVIDIA support verification failed."
        exit 1
    fi
}

# Function to perform system cleanup
cleanup() {
    info "Cleaning up unnecessary packages..."
    {
        apt autoremove -y
        apt autoclean -y
    } >> "$LOG_FILE" 2>&1
    success "Cleanup completed."
}

# =============================================================================
# Main Script Execution
# =============================================================================

# Redirect all output to log file
exec > >(tee -a "$LOG_FILE") 2>&1

# Display start message with timestamp
info "===== Script Execution Started at $(date) ====="

# Check for root privileges
check_root

# Check if the operating system is Ubuntu
check_os

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

# Verify Docker with NVIDIA support
verify_docker_nvidia

success "All tasks completed successfully!"

# Optionally, prompt to reboot if necessary
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

# Display end message with timestamp
info "===== Script Execution Ended at $(date) ====="

exit 0
