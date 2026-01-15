#!/bin/bash
set -eou pipefail

# Function to log with timestamp
log() {
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"
}

# Install bento from source
source ~/.cargo/env
log "Installing Bento from source..."
git clone https://github.com/boundless-xyz/boundless.git
cd boundless/
git submodule update --init --recursive
cd bento

# Set up CUDA environment
export LD_LIBRARY_PATH=/usr/local/cuda/lib64
export NVCC_APPEND_FLAGS="--generate-code arch=compute_75,code=sm_75 --generate-code arch=compute_86,code=sm_86 --generate-code arch=compute_89,code=sm_89 --generate-code arch=compute_120,code=sm_120"
export PATH=/usr/local/cuda/bin:$PATH

log "Installing RISC0 components..."
/home/ubuntu/.risc0/bin/rzup install

log "Building Bento agent with CUDA support (this may take 30-60 minutes)..."
# Use all available CPU cores for faster compilation
export CARGO_BUILD_JOBS=$(nproc)
cargo build --release -F cuda --bin agent

log "Building Bento REST API..."
cargo build --release --bin rest_api

log "Building Bento CLI..."
cargo build --release --bin bento_cli

log "Installing Bento binaries..."
sudo mv target/release/agent /usr/local/bin/agent
sudo mv target/release/rest_api /usr/local/bin/api
sudo mv target/release/bento_cli /usr/local/bin/bento_cli
sudo chmod +x /usr/local/bin/agent /usr/local/bin/api /usr/local/bin/bento_cli
cd ../

# Install Foundry
log "Installing Foundry..."
curl -L https://foundry.paradigm.xyz | bash
export PATH="$HOME/.foundry/bin:$PATH"
echo 'export PATH="$HOME/.foundry/bin:$PATH"' >> ~/.bashrc
foundryup

# Install broker from source
log "Building broker from source..."
forge build
cargo build --release --bin broker
sudo mv target/release/broker /usr/local/bin/broker
sudo chmod +x /usr/local/bin/broker

# Setup systemd files
log "Setting up systemd files..."
sudo mkdir -p /opt/boundless
sudo cp /tmp/bent*.service /etc/systemd/system/
sudo cp /tmp/broker.toml /opt/boundless/broker.toml

# Setup vector
log "Installing Vector..."
sudo bash -c "$(curl -L https://setup.vector.dev)"
sudo apt install vector -y
# Setup vector.toml
log "Setting up vector configuration..."
sudo cp /tmp/vector.yaml /etc/vector/vector.yaml
sudo systemctl enable vector

# Final cleanup
log "Performing final cleanup..."
# Clean up source code and build artifacts
log "Removing source repository and build artifacts..."
# Clean up Rust build cache (optional, but saves space)
log "Cleaning Rust build cache..."
# Running cargo clean is redundant here, but forwards compatible for if target dir is outside dir
cd ~/boundless/bento
cargo clean
cd ~
rm -rf boundless/
# Clean up apt cache and temporary files
sudo apt-get autoremove -y
sudo apt-get autoclean
sudo rm -rf /var/lib/apt/lists/*
sudo rm -rf /tmp/*
sudo rm -rf /var/tmp/*

log "All Bento dependencies installed successfully!"
