# Boundless Infrastructure with NixOS

This directory contains NixOS configurations for the Boundless infrastructure components with SOPS secrets management.

## 🔐 Secrets Management

This setup uses SOPS (Secrets OPerationS) for secure secrets management:

- **Encryption**: Age encryption for secrets
- **Integration**: Seamless integration with NixOS
- **Security**: Secrets are encrypted at rest and decrypted at runtime

### Setup Secrets

```bash
# Run the setup script
./setup-secrets.sh

# Edit secrets
sops secrets/secrets.yaml

# Verify secrets
sops -d secrets/secrets.yaml
```

## 📁 Structure

```
infra/nix/
├── flake.nix                 # Main flake configuration
├── modules/                  # NixOS modules
│   ├── config.nix           # Configuration options
│   ├── common.nix           # Common configuration
│   ├── secrets.nix          # SOPS secrets configuration
│   ├── manager.nix          # Manager instance
│   ├── prover.nix           # Prover instances
│   ├── execution.nix        # Execution instances
│   ├── aux.nix              # Aux instances
│   ├── broker.nix           # Broker instances
│   ├── postgresql.nix       # PostgreSQL instances
│   └── minio.nix            # MinIO instances
├── packages/                 # Custom packages
│   ├── manager.nix          # Manager package
│   ├── prover.nix           # Prover package
│   ├── execution.nix        # Execution package
│   ├── aux.nix              # Aux package
│   └── broker.nix           # Broker package
├── secrets/                  # Encrypted secrets (gitignored)
│   └── secrets.yaml         # Encrypted secrets file
├── setup-secrets.sh         # Secrets setup script
├── deploy.sh                # Deployment script
└── .gitignore              # Git ignore rules
```

## 🚀 Usage

### Prerequisites

```bash
# Install Nix with flakes support
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh -s -- install

# Enable flakes
mkdir -p ~/.config/nix
echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

### Setup

```bash
# 1. Setup secrets management
./setup-secrets.sh

# 2. Edit secrets with your actual values
sops secrets/secrets.yaml

# 3. Build configurations
nix build .#manager
nix build .#prover
```

### Building Configurations

```bash
# Build a specific configuration
nix build .#manager
nix build .#prover
nix build .#execution
nix build .#aux
nix build .#broker
nix build .#postgresql
nix build .#minio

# Build all configurations
nix build
```

### Deploying to a Host

```bash
# Deploy to a specific host
./deploy.sh manager
./deploy.sh prover
./deploy.sh execution
./deploy.sh aux
./deploy.sh broker
./deploy.sh postgresql
./deploy.sh minio
```

### Development

```bash
# Enter development shell
nix develop

# Build specific package
nix build .#manager
```

## ⚙️ Configuration

### Environment Variables

Each module can be customized by modifying the corresponding `.nix` file in the `modules/` directory. The `config.nix` module provides centralized configuration options.

### Secrets

Secrets are managed through SOPS and defined in `modules/secrets.nix`. To add a new secret:

1. Add it to `modules/secrets.nix`
2. Add it to `secrets/secrets.yaml`
3. Use it in your modules with `config.sops.secrets."secret_name".path`

### Network Configuration

Update IP addresses and ports in `modules/config.nix` or override them in specific modules.

## 🔒 Security

- **Secrets**: All secrets are encrypted with Age
- **Access Control**: Secrets are owned by the `boundless` user
- **File Permissions**: Secrets have restricted permissions (0400)
- **Git Safety**: Secrets directory is gitignored

## 🐛 Troubleshooting

### Common Issues

1. **SOPS not found**: Install SOPS and Age
2. **Secrets not decrypting**: Check age key path in `modules/secrets.nix`
3. **Build failures**: Ensure all dependencies are available
4. **Deployment failures**: Check SSH access and host connectivity

### Debug Commands

```bash
# Check flake configuration
nix flake check

# Show configuration options
nixos-rebuild show-configuration --flake .#manager

# Test configuration without applying
nixos-rebuild dry-run --flake .#manager
```

## 📚 Dependencies

- Nix with flakes support
- NixOS (for deployment)
- SOPS and Age for secrets management
- SSH access to target hosts
- Rust toolchain for building packages

## 🔄 Migration from Ansible

This Nix setup replaces the previous Ansible configuration with several advantages:

### ✅ **Reproducible**
- Same configuration always produces same result
- No "works on my machine" problems
- Deterministic builds

### ✅ **Atomic Updates**
- Entire system updates atomically
- Easy rollbacks
- No partial state corruption

### ✅ **Declarative**
- Describe what you want, not how to get there
- No imperative scripts to debug

### ✅ **Dependency Management**
- All dependencies explicitly declared
- No hidden dependencies
- Conflict resolution built-in

### ✅ **Security**
- All packages from trusted sources
- No arbitrary code execution
- Immutable system state
- Encrypted secrets management

### ✅ **Performance**
- Only rebuilds what changed
- Efficient binary caching
- Fast deployments
