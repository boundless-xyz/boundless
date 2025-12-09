# Boundless Ansible Setup

This Ansible playbook replicates the functionality of `scripts/setup.sh` for automated system provisioning.

## Overview

The playbook sets up a Boundless prover system by:

* Updating system packages
* Installing essential packages (nvtop, build tools, etc.)
* Installing GCC 12 for Ubuntu 22.04
* Installing Docker with NVIDIA Container Toolkit support
* Installing Rust programming language
* Installing Just command runner
* Installing CUDA Toolkit 13.0
* Installing Protobuf compiler
* Performing system cleanup

## Requirements

* Ansible 2.9 or later
* Target hosts running Ubuntu 22.04 or 24.04
* SSH access to target hosts (or run locally)
* Sudo/root privileges on target hosts

## Directory Structure

```
ansible/
├── playbook.yml          # Main playbook
├── inventory.yml          # Inventory file
├── ansible.cfg            # Ansible configuration
├── README.md              # This file
└── roles/
    ├── system/            # System update tasks
    ├── packages/          # Essential packages installation
    ├── gcc/               # GCC 12 installation (Ubuntu 22.04)
    ├── docker/            # Docker installation and configuration
    ├── nvidia-container-toolkit/  # NVIDIA Container Toolkit
    ├── rust/              # Rust installation
    ├── just/              # Just command runner
    ├── cuda/              # CUDA Toolkit installation
    ├── protobuf/          # Protobuf compiler
    └── cleanup/           # System cleanup
```

## Usage

### Running on localhost

```bash
ansible-playbook playbook.yml
```

### Running on remote hosts

1. Edit `inventory.yml` to add your target hosts:

```yaml
all:
  hosts:
    prover-1:
      ansible_host: 192.168.1.100
      ansible_user: ubuntu
      ansible_ssh_private_key_file: ~/.ssh/id_rsa
```

2. Run the playbook:

```bash
ansible-playbook playbook.yml
```

### Running on specific hosts

```bash
ansible-playbook playbook.yml -i inventory.yml --limit prover-1
```

### Running with tags (selective execution)

You can run specific roles using tags:

```bash
# Only install Docker
ansible-playbook playbook.yml --tags docker

# Install Docker and NVIDIA tools
ansible-playbook playbook.yml --tags docker,nvidia
```

## Differences from setup.sh

1. **Git submodules**: The Ansible playbook attempts to initialize git submodules, but this is best done manually or in a separate task since it requires the repository to be present.

2. **Interactive reboot**: The bash script prompts for reboot, but Ansible doesn't handle interactive prompts. You can reboot manually or add a separate task.

3. **Logging**: The bash script logs to `/var/log/setup.log`. Ansible has its own logging mechanisms.

## Notes

* The playbook is idempotent - you can run it multiple times safely
* Some tasks require a reboot to take full effect (especially GPU drivers and Docker group membership)
* For Ubuntu 22.04 vs 24.04, different installation methods are used for Rust and Just (apt vs direct install)
* Docker group membership changes require logging out and back in, or a reboot

## Troubleshooting

### Permission issues

Ensure your user has sudo privileges or run with `--ask-become-pass`:

```bash
ansible-playbook playbook.yml --ask-become-pass
```

### Connection issues

Test connectivity first:

```bash
ansible all -i inventory.yml -m ping
```

### Check what would change

Use `--check` for a dry run:

```bash
ansible-playbook playbook.yml --check
```
