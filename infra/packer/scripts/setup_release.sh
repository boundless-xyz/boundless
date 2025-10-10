#!/bin/bash
set -eou pipefail

# Download and install Bento binaries
echo "Installing Bento binaries..."
export BOUNDLESS_BENTO_VERSION=${BOUNDLESS_BENTO_VERSION:-"v1.0.1"}
echo "Using Bento version: $BOUNDLESS_BENTO_VERSION"
curl -L -o /tmp/bento-bundle.tar.gz "https://github.com/boundless-xyz/boundless/releases/download/bento-${BOUNDLESS_BENTO_VERSION}/bento-bundle-linux-amd64.tar.gz"
tar -xzf /tmp/bento-bundle.tar.gz -C /tmp
sudo mv /tmp/bento-bundle/bento-agent /usr/local/bin/agent
sudo mv /tmp/bento-bundle/bento-rest-api /usr/local/bin/api
sudo chmod +x /usr/local/bin/agent /usr/local/bin/api

# Download and install Broker binaries
echo "Installing Broker binary..."
export BOUNDLESS_BROKER_VERSION=${BOUNDLESS_BROKER_VERSION:-"v1.0.0"}
echo "Using Broker version: $BOUNDLESS_BROKER_VERSION"
wget "https://github.com/boundless-xyz/boundless/releases/download/broker-${BOUNDLESS_BROKER_VERSION}/broker" -O /tmp/broker
sudo mv /tmp/broker /usr/local/bin/broker
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
