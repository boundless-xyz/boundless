#!/bin/bash

# =============================================================================
# Script Name: setup.sh
# Description:
#   - Updates the system packages.
#   - Installs essential Boundless packages.
#   - Installs GPU drivers for provers.
#   - Installs Docker with NVIDIA support.
#   - Installs Rust programming language.
#   - Installs CUDA Toolkit.
#   - Performs system cleanup.
#   - Verifies Docker with NVIDIA support.
#
# Usage:
#   ./setup.sh [--help] [--no-prompt]
#
# Options:
#   --help        Show this help message and exit
#   --no-prompt   Run with default options (yes to all prompts)
# =============================================================================

# Parse command line arguments
NO_PROMPT=false

while [[ $# -gt 0 ]]; do
  case $1 in
    --help)
      echo "Usage: ./setup.sh [--help] [--no-prompt]"
      echo ""
      echo "Options:"
      echo "  --help        Show this help message and exit"
      echo "  --no-prompt   Run with default options (yes to all prompts)"
      exit 0
      ;;
    --no-prompt)
      NO_PROMPT=true
      shift
      ;;
    *)
      echo "Unknown option: $1"
      echo "Use --help for usage information"
      exit 1
      ;;
  esac
done

# Exit immediately if a command exits with a non-zero status,
# treat unset variables as an error, and propagate errors in pipelines.
set -euo pipefail

# =============================================================================
# Constants
# =============================================================================

SCRIPT_NAME="$(basename "$0")"

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

# Function to display warning messages
warning() {
    printf "\e[33m[WARNING]\e[0m %s\n" "$1"
}

# Function to check if a package is installed
is_package_installed() {
    dpkg -s "$1" &> /dev/null || return 1
}

# Function to prompt user for confirmation
# Returns: 0 for yes, 1 for no
confirm() {
    local prompt="$1"
    local default="${2:-N}"

    if [[ "$NO_PROMPT" == "true" ]]; then
        return 0
    fi

    if [[ "$default" == "Y" ]]; then
        local options="(Y/n)"
    else
        local options="(y/N)"
    fi

    read -rp "$prompt $options: " response
    response=${response:-$default}

    case "$response" in
        [yY][eE][sS]|[yY]) return 0 ;;
        *) return 1 ;;
    esac
}


# Function to check if the operating system is Ubuntu
check_os() {
    if [[ -f /etc/os-release ]]; then
        # Source the os-release file to get OS information
        # shellcheck source=/dev/null
        . /etc/os-release
        if [[ "${ID,,}" != "ubuntu" ]]; then
            error "Unsupported operating system: $NAME. This script is intended for Ubuntu."
            error "The script may not work correctly on your system."
            if confirm "Do you want to continue anyway?" "N"; then
                warning "Continuing with unsupported OS: $NAME. Some features may not work correctly."
            else
                info "Exiting as requested."
                exit 0
            fi
        elif [[ "${VERSION_ID,,}" != "22.04" && "${VERSION_ID,,}" != "20.04" ]]; then
            warning "Unsupported Ubuntu version: $VERSION. This script is optimized for Ubuntu 20.04 or 22.04."
            if confirm "Do you want to continue anyway?" "N"; then
                warning "Continuing with unsupported Ubuntu version: $VERSION. Some features may not work correctly."
            else
                info "Exiting as requested."
                exit 0
            fi
        else
            info "Operating System: $PRETTY_NAME (supported)"
        fi
    else
        error "/etc/os-release not found. Unable to determine the operating system."
        if confirm "Do you want to continue anyway?" "N"; then
            warning "Continuing without OS verification. Some features may not work correctly."
        else
            info "Exiting as requested."
            exit 0
        fi
    fi
}

# Function to update and upgrade the system
update_system() {
    info "Updating and upgrading the system packages..."
    info "This may take some time depending on your internet connection..."

    # Update the package list
    if ! sudo apt update -y; then
        error "Failed to update package list. Check your internet connection and try again."
        if confirm "Do you want to continue with the installation without updating?" "N"; then
            warning "Continuing without updating package list. Some installations may fail."
            return
        else
            exit 1
        fi
    fi

    # Upgrade packages
    if ! sudo apt upgrade -y; then
        error "Failed to upgrade packages. Continuing with the installation."
        warning "Some packages may not be up to date."
    fi

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

    # First check which packages are already installed to avoid unnecessary installations
    local to_install=()
    for pkg in "${packages[@]}"; do
        if ! is_package_installed "$pkg"; then
            to_install+=("$pkg")
        else
            info "Package '$pkg' is already installed. Skipping."
        fi
    done

    # If all packages are already installed, skip
    if [ ${#to_install[@]} -eq 0 ]; then
        success "All essential packages are already installed."
        return
    fi

    # Install missing packages
    info "Installing missing packages: ${to_install[*]}..."
    if ! sudo apt install -y "${to_install[@]}"; then
        error "Failed to install some packages. Continuing with the installation."
        warning "Some features may not work correctly."
    fi

    success "Essential packages installed successfully."
}

# Function to install GPU drivers
install_drivers() {
    info "Detecting and installing appropriate drivers..."
    info "This may take several minutes. Please be patient..."

    # Check if we have an NVIDIA GPU first
    if ! lspci | grep -i nvidia &> /dev/null; then
        warning "No NVIDIA GPU detected on this system."
        if confirm "Do you want to continue with driver installation anyway?" "N"; then
            warning "Continuing with driver installation despite no NVIDIA GPU detected."
        else
            info "Skipping driver installation as requested."
            return
        fi
    fi

    if ! sudo ubuntu-drivers install; then
        error "Failed to install GPU drivers automatically."
        if confirm "Do you want to continue without GPU drivers?" "N"; then
            warning "Continuing without GPU drivers. Some features may not work correctly."
            return
        else
            error "Exiting installation due to GPU driver installation failure."
            exit 1
        fi
    fi

    success "GPU drivers installed successfully."
    info "A system reboot is recommended after driver installation."
}

# Function to install Rust
install_rust() {
    if command -v rustc &> /dev/null; then
        local rust_version
        rust_version=$(rustc --version | cut -d ' ' -f 2)
        info "Rust is already installed (version $rust_version). Skipping Rust installation."

        # Optionally update existing Rust installation
        if confirm "Would you like to update Rust to the latest version?" "N"; then
            info "Updating Rust..."
            rustup update
            success "Rust updated successfully."
        fi
    else
        info "Installing Rust programming language..."
        info "This may take several minutes. Please be patient..."

        # Download and install Rust with more robust error handling
        if ! curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs > /tmp/rustup.sh; then
            error "Failed to download Rust installer. Check your internet connection."
            if confirm "Do you want to continue without Rust?" "N"; then
                warning "Continuing without Rust. You will not be able to build Rust components."
                return
            else
                exit 1
            fi
        fi

        # Make the installer executable and run it
        chmod +x /tmp/rustup.sh
        if ! /tmp/rustup.sh -y; then
            error "Rust installation failed."
            if confirm "Do you want to continue without Rust?" "N"; then
                warning "Continuing without Rust. You will not be able to build Rust components."
                return
            else
                exit 1
            fi
        fi

        # Clean up the installer
        rm -f /tmp/rustup.sh

        # Source Rust environment variables for the current session
        if [[ -f "$HOME/.cargo/env" ]]; then
            # shellcheck source=/dev/null
            source "$HOME/.cargo/env"
            success "Rust installed successfully."
            info "Installing additional Rust components for Boundless..."

            # Install additional components that might be useful
            rustup component add rustfmt clippy
        else
            error "Rust installation incomplete. ~/.cargo/env not found."
            if confirm "Do you want to continue without Rust?" "N"; then
                warning "Continuing without Rust. You will not be able to build Rust components."
                return
            else
                exit 1
            fi
        fi
    fi

    # Install RISC Zero toolchain if not already installed
    if ! command -v rzup &> /dev/null; then
        info "Installing RISC Zero toolchain (required for Boundless)..."
        if ! curl -L https://risczero.com/install | bash; then
            error "Failed to install RISC Zero toolchain."
            warning "You may need to install it manually later: curl -L https://risczero.com/install | bash"
        else
            # Install RISC Zero tools
            if command -v rzup &> /dev/null; then
                rzup install
                success "RISC Zero toolchain installed successfully."
            else
                warning "RISC Zero installer was downloaded but 'rzup' command is not available."
                warning "You may need to restart your terminal or install manually."
            fi
        fi
    else
        info "RISC Zero toolchain is already installed."
    fi
}

# Function to install CUDA Toolkit
install_cuda_toolkit() {
    # First check if NVIDIA GPU is available
    if ! lspci | grep -i nvidia &> /dev/null; then
        warning "No NVIDIA GPU detected. CUDA Toolkit is typically only useful with NVIDIA GPUs."
        if confirm "Do you want to install CUDA Toolkit anyway?" "N"; then
            warning "Continuing with CUDA Toolkit installation despite no NVIDIA GPU detected."
        else
            info "Skipping CUDA Toolkit installation as requested."
            return
        fi
    fi

    if is_package_installed "cuda-toolkit"; then
        info "CUDA Toolkit is already installed. Skipping CUDA installation."

        # Optionally check CUDA version
        if command -v nvcc &> /dev/null; then
            local cuda_version
            cuda_version=$(nvcc --version | grep "release" | awk '{print $6}' | cut -c2-)
            info "Installed CUDA version: $cuda_version"
        fi
    else
        info "Installing CUDA Toolkit and dependencies..."
        info "This is a large download and may take considerable time depending on your internet connection..."

        # Determine the correct distribution identifier for the package repository
        local distribution
        if ! distribution=$(grep '^ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"')$(grep '^VERSION_ID=' /etc/os-release | cut -d'=' -f2 | tr -d '"'| tr -d '\.'); then
            error "Failed to determine distribution for CUDA repository."
            if confirm "Do you want to continue without CUDA Toolkit?" "N"; then
                warning "Continuing without CUDA Toolkit. Some GPU-accelerated features may not work."
                return
            else
                exit 1
            fi
        fi

        info "Installing Nvidia CUDA keyring and repo for $distribution"

        # Download the CUDA keyring package
        if ! wget -q https://developer.download.nvidia.com/compute/cuda/repos/$distribution/"$(/usr/bin/uname -m)"/cuda-keyring_1.1-1_all.deb; then
            error "Failed to download CUDA keyring package. Check your internet connection."
            if confirm "Do you want to continue without CUDA Toolkit?" "N"; then
                warning "Continuing without CUDA Toolkit. Some GPU-accelerated features may not work."
                return
            else
                exit 1
            fi
        fi

        # Install the keyring package
        if ! sudo dpkg -i cuda-keyring_1.1-1_all.deb; then
            error "Failed to install CUDA keyring package."
            if confirm "Do you want to continue without CUDA Toolkit?" "N"; then
                warning "Continuing without CUDA Toolkit. Some GPU-accelerated features may not work."
                rm -f cuda-keyring_1.1-1_all.deb
                return
            else
                rm -f cuda-keyring_1.1-1_all.deb
                exit 1
            fi
        fi

        # Clean up the downloaded package
        rm -f cuda-keyring_1.1-1_all.deb

        # Update package lists
        if ! sudo apt-get update; then
            error "Failed to update package lists after adding CUDA repository."
            if confirm "Do you want to continue anyway?" "N"; then
                warning "Continuing despite repository update failure. CUDA installation may fail."
            else
                exit 1
            fi
        fi

        # Install CUDA toolkit
        if ! sudo apt-get install -y cuda-toolkit; then
            error "Failed to install CUDA Toolkit."
            if confirm "Do you want to continue without CUDA Toolkit?" "N"; then
                warning "Continuing without CUDA Toolkit. Some GPU-accelerated features may not work."
                return
            else
                exit 1
            fi
        fi

        # Set up environment variables
        if [ -d /usr/local/cuda/bin ]; then
            echo 'export PATH="/usr/local/cuda/bin:$PATH"' >> "$HOME/.bashrc"
            echo 'export LD_LIBRARY_PATH="/usr/local/cuda/lib64:$LD_LIBRARY_PATH"' >> "$HOME/.bashrc"
            info "Added CUDA environment variables to .bashrc"
            info "You'll need to restart your terminal or run 'source ~/.bashrc' to use CUDA."
        fi

        success "CUDA Toolkit installed successfully."
        info "A system reboot is recommended after CUDA installation."
    fi
}

# Function to install Docker
install_docker() {
    if command -v docker &> /dev/null; then
        local docker_version
        docker_version=$(docker --version | awk '{print $3}' | tr -d ',')
        info "Docker is already installed (version $docker_version). Skipping Docker installation."

        # Check if Docker service is running
        if ! systemctl is-active --quiet docker; then
            warning "Docker service is not running."
            if confirm "Do you want to start and enable the Docker service?" "Y"; then
                sudo systemctl start docker
                sudo systemctl enable docker
                success "Docker service started and enabled."
            fi
        else
            info "Docker service is running."
        fi
    else
        info "Installing Docker..."
        info "This may take several minutes. Please be patient..."

        # Install prerequisites
        if ! sudo apt install -y apt-transport-https ca-certificates curl gnupg-agent software-properties-common; then
            error "Failed to install Docker prerequisites."
            if confirm "Do you want to continue without Docker?" "N"; then
                warning "Continuing without Docker. Container features will not be available."
                return
            else
                exit 1
            fi
        fi

        # Add Docker's official GPG key - with better error handling
        if ! curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /usr/share/keyrings/docker-archive-keyring.gpg; then
            error "Failed to add Docker's GPG key."
            if confirm "Do you want to continue without Docker?" "N"; then
                warning "Continuing without Docker. Container features will not be available."
                return
            else
                exit 1
            fi
        fi

        # Set up the stable repository
        if ! echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/docker-archive-keyring.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable" | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null; then
            error "Failed to set up Docker repository."
            if confirm "Do you want to continue without Docker?" "N"; then
                warning "Continuing without Docker. Container features will not be available."
                return
            else
                exit 1
            fi
        fi

        # Update package index
        if ! sudo apt update -y; then
            error "Failed to update package lists after adding Docker repository."
            if confirm "Do you want to continue anyway?" "N"; then
                warning "Continuing despite repository update failure. Docker installation may fail."
            else
                exit 1
            fi
        fi

        # Install Docker Engine, CLI, and Containerd
        if ! sudo apt install -y docker-ce docker-ce-cli containerd.io; then
            error "Failed to install Docker packages."
            if confirm "Do you want to continue without Docker?" "N"; then
                warning "Continuing without Docker. Container features will not be available."
                return
            else
                exit 1
            fi
        fi

        # Enable Docker
        if ! sudo systemctl enable docker; then
            warning "Failed to enable Docker service at boot."
            if confirm "Do you want to continue anyway?" "Y"; then
                warning "Docker will need to be started manually after system reboot."
            fi
        fi

        # Start Docker Service
        if ! sudo systemctl start docker; then
            error "Failed to start Docker service."
            if confirm "Do you want to continue without Docker?" "N"; then
                warning "Continuing without active Docker service. Container features will not be available."
                warning "Try starting Docker manually after installation: sudo systemctl start docker"
                return
            else
                exit 1
            fi
        fi

        success "Docker installed and started successfully."
    fi
}

# Function to add user to Docker group
add_user_to_docker_group() {
    local username
    # Try to get the actual username, not root
    username=$(logname 2>/dev/null || echo "${SUDO_USER:-$USER}")

    # Fallback if we still can't determine the username
    if [ -z "$username" ] || [ "$username" = "root" ]; then
        warning "Could not determine the non-root username."
        if [ -n "$HOME" ] && [ "$HOME" != "/root" ]; then
            username=$(basename "$HOME")
            info "Using '$username' based on HOME directory."
        else
            error "Cannot determine the appropriate user to add to the Docker group."
            if confirm "Do you want to manually specify a username?" "Y"; then
                read -rp "Enter username to add to Docker group: " custom_username
                if [ -n "$custom_username" ]; then
                    username="$custom_username"
                else
                    warning "No username provided. Skipping Docker group modification."
                    return
                fi
            else
                warning "Skipping Docker group modification."
                return
            fi
        fi
    fi

    # Verify the user exists
    if ! id "$username" &>/dev/null; then
        error "User '$username' does not exist."
        if confirm "Do you want to manually specify a username?" "Y"; then
            read -rp "Enter username to add to Docker group: " custom_username
            if [ -n "$custom_username" ] && id "$custom_username" &>/dev/null; then
                username="$custom_username"
            else
                warning "Invalid username. Skipping Docker group modification."
                return
            fi
        else
            warning "Skipping Docker group modification."
            return
        fi
    fi

    if id -nG "$username" | grep -qw "docker"; then
        info "User '$username' is already in the 'docker' group."
    else
        info "Adding user '$username' to the 'docker' group..."
        if ! sudo usermod -aG docker "$username"; then
            error "Failed to add user '$username' to the Docker group."
            warning "You may need to run Docker commands with sudo, or add the user manually: sudo usermod -aG docker $username"
            return
        fi
        success "User '$username' added to the 'docker' group."
        warning "Important: You must log out and log back in for the group change to take effect."
        warning "Until then, you may need to use 'sudo docker' to run Docker commands."
    fi
}

# Function to install NVIDIA Container Toolkit
install_nvidia_container_toolkit() {
    info "Checking NVIDIA Container Toolkit installation..."

    # Check if NVIDIA GPU is available first
    if ! lspci | grep -i nvidia &> /dev/null; then
        warning "No NVIDIA GPU detected on this system."
        if confirm "Do you want to continue with NVIDIA Container Toolkit installation anyway?" "N"; then
            warning "Continuing with installation despite no NVIDIA GPU detected."
        else
            info "Skipping NVIDIA Container Toolkit installation as requested."
            return
        fi
    fi

    if is_package_installed "nvidia-container-toolkit"; then
        success "NVIDIA Container Toolkit is already installed."
        return
    fi

    info "Installing NVIDIA Container Toolkit..."
    info "This may take several minutes. Please be patient..."

    # Use the official installation method that is maintained
    # Add the repository and GPG key in a more modern way
    if ! curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | sudo gpg --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg; then
        error "Failed to add NVIDIA Container Toolkit GPG key."
        if confirm "Do you want to continue without NVIDIA Container Toolkit?" "N"; then
            warning "Continuing without NVIDIA Container Toolkit. GPU support in containers will not be available."
            return
        else
            exit 1
        fi
    fi

    # Get the distribution information and set up the repository
    distribution=$(. /etc/os-release;echo $ID$VERSION_ID)
    if ! curl -s -L "https://nvidia.github.io/libnvidia-container/$distribution/libnvidia-container.list" | \
        sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' | \
        sudo tee /etc/apt/sources.list.d/nvidia-container-toolkit.list; then
        error "Failed to add NVIDIA Container Toolkit repository."
        if confirm "Do you want to continue without NVIDIA Container Toolkit?" "N"; then
            warning "Continuing without NVIDIA Container Toolkit. GPU support in containers will not be available."
            return
        else
            exit 1
        fi
    fi

    # Update the package lists
    if ! sudo apt update -y; then
        error "Failed to update package lists after adding NVIDIA Container Toolkit repository."
        if confirm "Do you want to continue anyway?" "N"; then
            warning "Continuing despite repository update failure. NVIDIA Container Toolkit installation may fail."
        else
            exit 1
        fi
    fi

    # Install the NVIDIA Container Toolkit
    if ! sudo apt install -y nvidia-container-toolkit; then
        error "Failed to install NVIDIA Container Toolkit."
        if confirm "Do you want to continue without NVIDIA Container Toolkit?" "N"; then
            warning "Continuing without NVIDIA Container Toolkit. GPU support in containers will not be available."
            return
        else
            exit 1
        fi
    fi

    # Configure the runtime
    if ! sudo nvidia-ctk runtime configure --runtime=docker; then
        error "Failed to configure Docker runtime with NVIDIA Container Toolkit."
        warning "You may need to configure it manually: sudo nvidia-ctk runtime configure --runtime=docker"
    fi

    # Restart Docker to apply changes
    if ! sudo systemctl restart docker; then
        error "Failed to restart Docker service after installing NVIDIA Container Toolkit."
        warning "You may need to restart Docker manually: sudo systemctl restart docker"
    fi

    success "NVIDIA Container Toolkit installed successfully."
}

# Function to configure Docker daemon for NVIDIA
configure_docker_nvidia() {
    info "Configuring Docker to use NVIDIA runtime by default..."

    # Check if Docker is installed first
    if ! command -v docker &> /dev/null; then
        warning "Docker is not installed. Skipping NVIDIA Docker configuration."
        return
    fi

    # Check if nvidia-container-toolkit is installed
    if ! is_package_installed "nvidia-container-toolkit"; then
        warning "NVIDIA Container Toolkit is not installed. Skipping NVIDIA Docker configuration."
        if confirm "Do you want to install NVIDIA Container Toolkit first?" "Y"; then
            install_nvidia_container_toolkit
        else
            return
        fi
    fi

    # Create Docker daemon configuration directory if it doesn't exist
    sudo mkdir -p /etc/docker

    # Handle the daemon.json file more safely to preserve user customizations
    if [ -f /etc/docker/daemon.json ]; then
        # Back up the file
        sudo cp /etc/docker/daemon.json /etc/docker/daemon.json.backup."$(date +%Y%m%d%H%M%S)"
        info "Backed up existing Docker daemon configuration."

        # Read the existing configuration and merge it with our changes
        local temp_file=$(mktemp)
        if ! jq '.["default-runtime"] = "nvidia" | .runtimes += {"nvidia": {"path": "nvidia-container-runtime", "runtimeArgs": []}}' /etc/docker/daemon.json > "$temp_file"; then
            error "Failed to update Docker daemon configuration. Could not parse existing config."
            rm -f "$temp_file"
            if confirm "Do you want to overwrite the existing configuration?" "N"; then
                # Create new configuration
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
            else
                warning "Skipping Docker NVIDIA configuration."
                return
            fi
        else
            # Use the merged configuration
            sudo cp "$temp_file" /etc/docker/daemon.json
            rm -f "$temp_file"
        fi
    else
        # Create new configuration
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
    fi

    # Restart Docker to apply the new configuration
    if ! sudo systemctl restart docker; then
        error "Failed to restart Docker service after configuring NVIDIA runtime."
        warning "You may need to restart Docker manually: sudo systemctl restart docker"
        warning "Or reboot your system for changes to take effect."
        return
    fi

    success "Docker configured to use NVIDIA runtime by default."
}

# Function to verify Docker with NVIDIA support
verify_docker_nvidia() {
    info "Verifying Docker and NVIDIA setup..."

    # Check if Docker is installed first
    if ! command -v docker &> /dev/null; then
        warning "Docker is not installed. Skipping verification of NVIDIA Docker support."
        return
    fi

    # Check if nvidia-container-toolkit is installed
    if ! is_package_installed "nvidia-container-toolkit"; then
        warning "NVIDIA Container Toolkit is not installed. Skipping verification of NVIDIA Docker support."
        return
    fi

    # Use a more recent CUDA image that's likely to be compatible with newer drivers
    if ! docker run --rm --gpus all nvidia/cuda:12.2.0-base-ubuntu22.04 nvidia-smi; then
        error "Docker with NVIDIA support verification failed."
        warning "GPU acceleration in containers may not be working correctly."
        warning "You may need to reboot your system for all changes to take effect."

        if confirm "Would you like to continue anyway?" "Y"; then
            warning "Continuing despite NVIDIA Docker verification failure."
            return
        else
            exit 1
        fi
    fi

    success "Docker with NVIDIA support is working correctly."
}

# Function to perform system cleanup
cleanup() {
    info "Cleaning up unnecessary packages..."
    sudo apt autoremove -y
    sudo apt autoclean -y
    success "Cleanup completed."
}

init_git_submodules() {
    # Check if git is installed
    if ! command -v git &> /dev/null; then
        warning "Git is not installed. Skipping submodule initialization."
        if confirm "Do you want to install Git?" "Y"; then
            sudo apt update
            sudo apt install -y git
            success "Git installed successfully."
        else
            warning "Continuing without Git. Some components may not be available."
            return
        fi
    fi

    # Check if we're in a git repository
    if ! git rev-parse --is-inside-work-tree &> /dev/null; then
        warning "Current directory is not a Git repository. Skipping submodule initialization."
        return
    fi

    info "Ensuring Git submodules are initialized..."
    if ! git submodule update --init --recursive; then
        error "Failed to initialize Git submodules."
        if confirm "Do you want to continue anyway?" "N"; then
            warning "Continuing without Git submodules. Some components may not be available."
            return
        else
            exit 1
        fi
    fi
    success "Git submodules initialized successfully."
}

# =============================================================================
# Main Script Execution
# =============================================================================

# Display start message with timestamp
info "===== Boundless Setup Started at $(date) ====="

# Check if the operating system is Ubuntu
check_os

# Ensure all the required source code is present
init_git_submodules

# Update and upgrade the system
if confirm "Do you want to update and upgrade the system?" "N"; then
    update_system
else
    info "Skipping system update and upgrade."
fi

# Install essential packages
install_packages

# Install GPU drivers
if confirm "Do you want to install NVIDIA GPU drivers and CUDA Toolkit?" "N"; then
    install_drivers
    if confirm "Do you want to install CUDA Toolkit?" "N"; then
        install_cuda_toolkit
    else
        info "Skipping CUDA Toolkit installation."
    fi
else
    info "Skipping GPU drivers installation."
fi

# Install Docker
if confirm "Do you want to install Docker?" "N"; then
    install_docker
    add_user_to_docker_group

    # Only configure NVIDIA Docker if Docker was installed
    if confirm "Do you want to install NVIDIA Container Toolkit for GPU support in Docker?" "N"; then
        install_nvidia_container_toolkit
        configure_docker_nvidia
        # Verify Docker with NVIDIA support
        verify_docker_nvidia
    else
        info "Skipping NVIDIA Container Toolkit installation."
    fi
else
    info "Skipping Docker installation."
fi

# Install Rust
if confirm "Do you want to install or update Rust and RISC Zero tools?" "Y"; then
    install_rust
else
    info "Skipping Rust installation/update."
fi

# Cleanup
if confirm "Do you want to clean up unnecessary packages?" "Y"; then
    cleanup
else
    info "Skipping cleanup."
fi

success "Boundless setup completed successfully!"

# Summary of installed components
info "Summary of installed components:"
echo "----------------------------------------"
if command -v rustc &> /dev/null; then
    echo "✅ Rust: $(rustc --version)"
else
    echo "❌ Rust: Not installed"
fi

if command -v rzup &> /dev/null; then
    echo "✅ RISC Zero: Installed"
else
    echo "❌ RISC Zero: Not installed"
fi

if command -v docker &> /dev/null; then
    echo "✅ Docker: $(docker --version)"
else
    echo "❌ Docker: Not installed"
fi

if is_package_installed "nvidia-container-toolkit"; then
    echo "✅ NVIDIA Container Toolkit: Installed"
else
    echo "❌ NVIDIA Container Toolkit: Not installed"
fi

if command -v nvcc &> /dev/null; then
    echo "✅ CUDA: $(nvcc --version | grep release | awk '{print $6}' | cut -c2-)"
else
    echo "❌ CUDA: Not installed"
fi
echo "----------------------------------------"

# Optionally, prompt to reboot if necessary
if command -v nvidia-smi &> /dev/null || is_package_installed "cuda-toolkit" || is_package_installed "nvidia-container-toolkit"; then
    warning "A system reboot is STRONGLY recommended to ensure all GPU-related changes take effect."
    if confirm "Do you want to reboot now?" "Y"; then
        info "Rebooting the system..."
        sudo reboot
    else
        warning "Reboot skipped. Please reboot your system manually to apply all changes."
    fi
else
    if confirm "Do you want to reboot now to apply all changes?" "N"; then
        info "Rebooting the system..."
        sudo reboot
    else
        info "Reboot skipped. Consider rebooting your system if you encounter any issues."
    fi
fi

# Display end message with timestamp
info "===== Boundless Setup Ended at $(date) ====="

exit 0
