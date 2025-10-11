#!/bin/bash
set -eou pipefail

# Install bento from source
source ~/.cargo/env
echo "Installing Bento from source..."
git clone https://github.com/boundless-xyz/boundless.git
git submodule update --init --recursive
cd boundless/bento
export LD_LIBRARY_PATH=/usr/local/cuda/lib64
export NVCC_APPEND_FLAGS="--generate-code arch=compute_75,code=sm_75 --generate-code arch=compute_86,code=sm_86 --generate-code arch=compute_89,code=sm_89 --generate-code arch=compute_120,code=sm_120"
export PATH=/usr/local/cuda/bin:$PATH
/home/ubuntu/.risc0/bin/rzup install
cargo build --release -F cuda --bin agent
cargo build --release --bin rest_api
cargo build --release --bin bento_cli
sudo mv target/release/agent /usr/local/bin/agent
sudo mv target/release/rest_api /usr/local/bin/api
sudo mv target/release/bento_cli /usr/local/bin/bento_cli
sudo chmod +x /usr/local/bin/agent /usr/local/bin/api /usr/local/bin/bento_cli
cd ../
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
