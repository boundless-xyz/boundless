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
COMPLETION_MARKER="/var/log/${SCRIPT_NAME%.sh}.completed"

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

# Function to check if setup has already been completed
check_completion() {
    if [[ -f "$COMPLETION_MARKER" ]]; then
        info "Setup has already been completed on $(cat "$COMPLETION_MARKER")"
        info "To run setup again, remove the completion marker: sudo rm $COMPLETION_MARKER"
        exit 0
    fi
}

# Function to display risk warning and get user acceptance
display_risk_warning() {
    echo
    echo "╔══════════════════════════════════════════════════════════════════════════╗"
    echo "║                              RISK WARNING                                ║"
    echo "╠══════════════════════════════════════════════════════════════════════════╣"
    echo "║                                                                          ║"
    echo "║  This script will make SIGNIFICANT changes to your system:               ║"
    echo "║                                                                          ║"
    echo "║  • Install/overwrite NVIDIA GPU drivers (may break existing drivers)     ║"
    echo "║  • Modify Docker configuration and daemon settings                       ║"
    echo "║  • Install CUDA toolkit and NVIDIA Container Toolkit                     ║"
    echo "║  • Update system packages and install additional software                ║"
    echo "║  • Modify user groups and permissions                                    ║"
    echo "║                                                                          ║"
    echo "║  THESE CHANGES MAY:                                                      ║"
    echo "║     • Break existing GPU drivers or CUDA installations                   ║"
    echo "║     • Overwrite custom Docker configurations                             ║"
    echo "║     • Require system reboot to function properly                         ║"
    echo "║     • Cause system instability if hardware is incompatible               ║"
    echo "║                                                                          ║"
    echo "║  USE AT YOUR OWN RISK! BACKUP IMPORTANT DATA FIRST!                      ║"
    echo "║                                                                          ║"
    echo "╚══════════════════════════════════════════════════════════════════════════╝"
    echo

    if [[ -t 0 ]]; then
        # We're in an interactive terminal
        echo "Do you understand and accept ALL risks associated with this setup?"
        echo "Type 'I ACCEPT ALL RISKS' (exactly) to continue, or anything else to abort:"
        read -r response

        if [[ "$response" != "I ACCEPT ALL RISKS" ]]; then
            error "Setup aborted by user. No changes were made."
            exit 1
        fi

        echo
        info "Risk acceptance confirmed. Proceeding with setup..."
        echo
    else
        # We're in a non-interactive environment (like EC2 user data)
        error "Cannot run interactively. This script requires explicit risk acceptance."
        error "To run non-interactively, set environment variable: ACCEPT_RISKS=true"
        if [[ "${ACCEPT_RISKS:-}" != "true" ]]; then
            exit 1
        fi
        info "Non-interactive risk acceptance confirmed via environment variable."
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
        elif [[ "${VERSION_ID,,}" != "22.04" && "${VERSION_ID,,}" != "24.04" ]]; then
            error "Unsupported operating system version: $VERSION. This script is intended for Ubuntu 22.04 or 24.04."
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

# Function to install specific GCC version for Ubuntu 22.04
install_gcc_version() {
    # Get Ubuntu version
    local ubuntu_version
    ubuntu_version=$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')

    if [[ "$ubuntu_version" == "22.04" ]]; then
        info "Installing GCC 12 for Ubuntu 22.04..."
        {
            sudo apt install -y gcc-12 g++-12
            # Set gcc-12 and g++-12 as alternatives with higher priority
            sudo update-alternatives --install /usr/bin/gcc gcc /usr/bin/gcc-12 100
            sudo update-alternatives --install /usr/bin/g++ g++ /usr/bin/g++-12 100
        } >> "$LOG_FILE" 2>&1
        success "GCC 12 installed and configured successfully."
    else
        info "Skipping GCC 12 installation (not needed for Ubuntu $ubuntu_version)."
    fi
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

# Function to install the `just` command runner
install_just() {
    if command -v just &>/dev/null; then
        info "'just' is already installed. Skipping."
        return
    fi

    info "Installing the 'just' command-runner…"
    {
        # Install the latest pre-built binary straight to /usr/local/bin
        curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
        | sudo bash -s -- --to /usr/local/bin
    } >> "$LOG_FILE" 2>&1

    success "'just' installed successfully."
}

# Function to install CUDA Toolkit
install_cuda() {
    if is_package_installed "cuda-toolkit-13-0" && is_package_installed "nvidia-open"; then
        info "CUDA Toolkit and nvidia-open are already installed. Skipping CUDA installation."
    else
        info "Installing CUDA Toolkit and dependencies..."
        {
            # Get Ubuntu version for CUDA repository
            local ubuntu_version
            ubuntu_version=$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')

            # Map Ubuntu versions to CUDA repository versions
            local cuda_repo_version
            case "$ubuntu_version" in
                "22.04")
                    cuda_repo_version="ubuntu2204"
                    ;;
                "24.04")
                    cuda_repo_version="ubuntu2404"
                    ;;
                *)
                    error "Unsupported Ubuntu version: $ubuntu_version"
                    exit 1
                    ;;
            esac

            info "Installing Nvidia CUDA keyring and repo for $cuda_repo_version"
            wget https://developer.download.nvidia.com/compute/cuda/repos/$cuda_repo_version/x86_64/cuda-keyring_1.1-1_all.deb
            sudo dpkg -i cuda-keyring_1.1-1_all.deb
            rm cuda-keyring_1.1-1_all.deb
            sudo apt-get update
            sudo apt-get -y install cuda-toolkit-13-0 nvidia-open
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
    username=$(logname 2>/dev/null || echo "${SUDO_USER:-$(whoami)}")

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

    if is_package_installed "nvidia-container-toolkit"; then
        success "NVIDIA Container Toolkit is already installed."
        return
    fi

    info "Installing NVIDIA Container Toolkit..."

    {
        # Get Ubuntu version for NVIDIA Container Toolkit repository
        local ubuntu_version
        ubuntu_version=$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')

        # Map Ubuntu versions to NVIDIA Container Toolkit repository versions
        local nvidia_repo_version
        case "$ubuntu_version" in
            "22.04")
                nvidia_repo_version="ubuntu22.04"
                ;;
            "24.04")
                nvidia_repo_version="ubuntu24.04"
                ;;
            *)
                error "Unsupported Ubuntu version: $ubuntu_version"
                exit 1
                ;;
        esac

        info "Installing NVIDIA Container Toolkit for $nvidia_repo_version"

        # Add NVIDIA Container Toolkit GPG key and repository
        curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
        curl -s -L https://nvidia.github.io/libnvidia-container/stable/deb/nvidia-container-toolkit.list | \
            sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' | \
            sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list

        # Update the package lists
        sudo apt update -y

        # Install the NVIDIA Container Toolkit
        sudo apt install -y nvidia-container-toolkit

        # Configure Docker to use NVIDIA runtime
        sudo nvidia-ctk runtime configure --runtime=docker
        sudo systemctl restart docker
    } >> "$LOG_FILE" 2>&1

    success "NVIDIA Container Toolkit installed successfully."
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

# Check if setup has already been completed
check_completion

# Display risk warning and get user acceptance
display_risk_warning

# ensure all the require source code is present
init_git_submodules

# Update and upgrade the system
update_system

# Install essential packages
install_packages

# Install specific GCC version for Ubuntu 22.04
install_gcc_version

# Install Docker
install_docker

# Add user to Docker group
add_user_to_docker_group

# Install NVIDIA Container Toolkit
install_nvidia_container_toolkit

# Install Rust
install_rust

# Install Just
install_just

# Install CUDA Toolkit
install_cuda

# Cleanup
cleanup

# Create completion marker
info "Creating completion marker..."
echo "$(date)" > "$COMPLETION_MARKER"
success "Setup completed successfully! Marker created at $COMPLETION_MARKER"

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
