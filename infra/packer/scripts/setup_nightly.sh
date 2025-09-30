#!/bin/bash
set -eou pipefail
echo "Installing all Bento dependencies..."

# Update system
sudo apt-get update -y
sudo apt-get upgrade -y
sudo apt-get install -y jq htop tree git nvtop build-essential pkg-config libssl-dev curl wget gnupg2 software-properties-common apt-transport-https ca-certificates lsb-release protobuf-compiler unzip

# Install NVIDIA drivers and CUDA
echo "Installing NVIDIA drivers and CUDA..."
wget https://developer.download.nvidia.com/compute/cuda/repos/ubuntu2404/x86_64/cuda-keyring_1.1-1_all.deb
sudo dpkg -i cuda-keyring_1.1-1_all.deb
sudo apt-get update
sudo apt-get -y install cuda-toolkit-13-0 nvidia-open

# Install AWS CLI v2
echo "Installing AWS CLI v2..."
curl "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o "awscliv2.zip"
unzip awscliv2.zip
sudo ./aws/install
rm -rf aws awscliv2.zip

# Install SSM Agent

echo "Installing SSM Agent..."
sudo snap install amazon-ssm-agent --classic
sudo systemctl enable snap.amazon-ssm-agent.amazon-ssm-agent.service

# Install CloudWatch Agent
echo "Installing CloudWatch Agent..."
wget https://s3.amazonaws.com/amazoncloudwatch-agent/ubuntu/amd64/latest/amazon-cloudwatch-agent.deb
sudo dpkg -i -E ./amazon-cloudwatch-agent.deb
rm amazon-cloudwatch-agent.deb

# Install Rust
echo "Installing Rust..."
curl https://sh.rustup.rs -sSf | sh -s -- -y

# Source the cargo environment
source ~/.cargo/env

# Install RISC0
echo "Installing RISC0..."
curl -L https://risczero.com/install | bash

# Install RISC0 Groth16
echo "Installing RISC0 Groth16..."
/home/ubuntu/.risc0/bin/rzup install risc0-groth16

# Install bento from source
echo "Installing Bento from source..."
git clone https://github.com/boundless-xyz/boundless.git
cd boundless/bento
cargo build --release -F cuda --bin agent
cargo build --release --bin rest_api
cargo build --release --bin bento_cli
sudo mv target/release/agent /usr/local/bin/agent
sudo mv target/release/rest_api /usr/local/bin/api
sudo mv target/release/bento_cli /usr/local/bin/bento_cli
sudo chmod +x /usr/local/bin/agent /usr/local/bin/api /usr/local/bin/bento_cli

# Install broker from source
cargo build --release --bin broker
sudo mv target/release/broker /usr/local/bin/broker
sudo chmod +x /usr/local/bin/broker

# Setup systemd files
echo "Setting up systemd files..."
sudo mkdir -p /opt/boundless
sudo cp /tmp/bent*.service /etc/systemd/system/
sudo cp /tmp/broker.toml /opt/boundless/broker.toml

# Setup vector
sudo bash -c "$(curl -L https://setup.vector.dev)"
sudo apt install vector -y
# Setup vector.toml
echo "Setting up vector.toml..."
sudo cp /tmp/vector.yaml /etc/vector/vector.yaml
sudo systemctl enable vector

# Final cleanup
echo "Performing final cleanup..."
sudo apt-get autoremove -y
sudo apt-get autoclean
sudo rm -rf /var/lib/apt/lists/*
sudo rm -rf /tmp/*
sudo rm -rf /var/tmp/*

echo "All Bento dependencies installed successfully!"
