#!/bin/bash
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

# Download and install Bento binaries
echo "Installing Bento binaries..."
curl -L -o /tmp/bento-bundle.tar.gz 'https://github.com/boundless-xyz/boundless/releases/download/bento-v1.0.1/bento-bundle-linux-amd64.tar.gz'
tar -xzf /tmp/bento-bundle.tar.gz -C /tmp
sudo mv /tmp/bento-bundle/bento-agent /usr/local/bin/agent
sudo mv /tmp/bento-bundle/bento-rest-api /usr/local/bin/api
sudo chmod +x /usr/local/bin/agent /usr/local/bin/api

# Download and install Broker binaries
echo "Installing Broker binary..."
wget https://github.com/boundless-xyz/boundless/releases/download/broker-v1.0.0/broker -O /tmp/broker
sudo mv /tmp/broker /usr/local/bin/broker
sudo chmod +x /usr/local/bin/broker

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

# Setup systemd files
echo "Setting up systemd files..."
sudo mkdir -p /opt/boundless
sudo cp /tmp/bent*.service /etc/systemd/system/
sudo cp /tmp/broker.toml /opt/boundless/broker.toml

# Final cleanup
echo "Performing final cleanup..."
sudo apt-get autoremove -y
sudo apt-get autoclean
sudo rm -rf /var/lib/apt/lists/*
sudo rm -rf /tmp/*
sudo rm -rf /var/tmp/*

echo "All Bento dependencies installed successfully!"
