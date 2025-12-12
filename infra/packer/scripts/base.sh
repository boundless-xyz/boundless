#!/bin/bash
set -eou pipefail
echo "Installing all Bento dependencies..."

# Update system
sudo apt-get update -y
sudo apt-get install -y jq htop tree git nvtop build-essential pkg-config libssl-dev curl wget gnupg2 software-properties-common apt-transport-https ca-certificates lsb-release protobuf-compiler unzip clang
# journald will handle logging, so we can remove rsyslog
sudo apt-get remove rsyslog -y
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
# Configure CloudWatch Agent
echo "Setting up CloudWatch Agent..."
sudo cp /tmp/cloudwatch-agent.json /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json
sudo systemctl enable amazon-cloudwatch-agent

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
