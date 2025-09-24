#!/usr/bin/env bash
set -euo pipefail

# Setup script for SOPS secrets management
# This script helps you set up SOPS for the Boundless infrastructure

echo "🔐 Setting up SOPS secrets management for Boundless infrastructure..."

# Check if SOPS is installed
if ! command -v sops &> /dev/null; then
    echo "❌ SOPS is not installed. Please install it first:"
    echo "   - macOS: brew install sops"
    echo "   - Linux: nix-env -iA nixpkgs.sops"
    echo "   - Or use the Nix flake: nix develop"
    exit 1
fi

# Check if age is installed
if ! command -v age &> /dev/null; then
    echo "❌ Age is not installed. Please install it first:"
    echo "   - macOS: brew install age"
    echo "   - Linux: nix-env -iA nixpkgs.age"
    echo "   - Or use the Nix flake: nix develop"
    exit 1
fi

# Create secrets directory if it doesn't exist
mkdir -p secrets

# Generate age key if it doesn't exist
if [ ! -f "secrets/age-key.txt" ]; then
    echo "🔑 Generating new age key..."
    age-keygen -o secrets/age-key.txt
    echo "✅ Age key generated: secrets/age-key.txt"
    echo "⚠️  IMPORTANT: Keep this key secure and never commit it to version control!"
else
    echo "✅ Age key already exists: secrets/age-key.txt"
fi

# Extract the public key
AGE_PUBLIC_KEY=$(grep "public key:" secrets/age-key.txt | cut -d' ' -f4)
echo "🔑 Public key: $AGE_PUBLIC_KEY"

# Create .sops.yaml configuration
cat > .sops.yaml << EOF
# SOPS configuration for Boundless infrastructure
creation_rules:
  - path_regex: secrets/.*\.yaml$
    age: $AGE_PUBLIC_KEY
    encrypted_regex: '^(database_password|ethereum_private_key|minio_access_key|minio_secret_key|redis_password|jwt_secret|aws_access_key_id|aws_secret_access_key)$'
EOF

echo "✅ Created .sops.yaml configuration"

# Create encrypted secrets file if it doesn't exist
if [ ! -f "secrets/secrets.yaml" ]; then
    echo "🔐 Creating encrypted secrets file..."
    sops --encrypt --age "$AGE_PUBLIC_KEY" secrets.yaml > secrets/secrets.yaml
    echo "✅ Encrypted secrets file created: secrets/secrets.yaml"
else
    echo "✅ Encrypted secrets file already exists: secrets/secrets.yaml"
fi

# Update the secrets module to use the correct path
echo "📝 Updating secrets module to use correct path..."
sed -i.bak "s|./secrets.yaml|./secrets/secrets.yaml|g" modules/secrets.nix
rm -f modules/secrets.nix.bak

echo ""
echo "🎉 SOPS setup complete!"
echo ""
echo "Next steps:"
echo "1. Edit secrets/secrets.yaml with your actual secrets:"
echo "   sops secrets/secrets.yaml"
echo ""
echo "2. Update the age key path in modules/secrets.nix if needed"
echo ""
echo "3. Deploy your configuration:"
echo "   nixos-rebuild switch --flake .#manager"
echo ""
echo "⚠️  Security reminders:"
echo "- Never commit secrets/age-key.txt to version control"
echo "- Add secrets/ to your .gitignore"
echo "- Keep your age key secure and backed up"
echo "- Rotate secrets regularly"
