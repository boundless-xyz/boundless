# Boundless Packer AMI Builds

This directory contains Packer configurations for building Amazon Machine Images (AMIs) for the Boundless Bento infrastructure.

## Overview

The Packer setup provides:

- **Unified AMI builds**: Single AMI for both manager and prover nodes
- **Cross-account sharing**: AMIs automatically shared with service accounts
- **Version control**: Independent versioning for Bento and Broker components
- **GPU support**: NVIDIA CUDA and GPU drivers pre-installed
- **Service-ready**: All Bento services pre-configured

## Directory Structure

```
packer/
├── bento.pkr.hcl              # Main AMI build configuration
├── variables.pkr.hcl          # Variable definitions
├── config_files/              # Service configuration files
│   ├── vector.yaml            # Vector logging configuration
├── service_files/             # Systemd service files
│   ├── bento-api.service
│   ├── bento-broker.service
│   ├── bento-executor.service
│   ├── bento-aux.service
│   ├── bento-prover.service
│   └── broker.toml
├── scripts/                   # Installation scripts
│   └── setup.sh               # Main setup script
└── README.md                  # This file
```

## AMI Configuration

### **Unified AMI** (`bento.pkr.hcl`)

- **Purpose**: General-purpose AMI for both manager and prover nodes
- **Instance Type**: Configurable (default: c7a.4xlarge)
- **Services**: All Bento services (API, Broker, Executor, Aux, Prover)
- **GPU**: Optional, based on instance type
- **Logging**: Vector configured for CloudWatch log forwarding
- **Base Image**: Ubuntu 24.04 with NVIDIA drivers

## Usage

### **Local Development**

1. **Install Packer**:
   ```bash
   # Download and install Packer
   wget https://releases.hashicorp.com/packer/1.9.4/packer_1.9.4_linux_amd64.zip
   unzip packer_1.9.4_linux_amd64.zip
   sudo mv packer /usr/local/bin/
   ```

2. **Configure AWS credentials**:
   ```bash
   aws configure
   # Or use AWS_PROFILE environment variable
   export AWS_PROFILE=boundless-ops
   ```

3. **Build AMI locally**:
   ```bash
   cd infra/packer

   # Build unified AMI
   packer build -var "boundless_bento_version=v1.0.1" bento.pkr.hcl

   # Build with custom instance type
   packer build -var "boundless_bento_version=v1.0.1" -var "instance_type=g6.xlarge" bento.pkr.hcl

   # Build with specific Bento and Broker versions
   packer build -var "boundless_bento_version=v1.0.1" -var "boundless_broker_version=v1.0.0" bento.pkr.hcl

   # Build for specific environment
   packer build -var "boundless_bento_version=v1.0.1" -var "environment=staging" bento.pkr.hcl
   ```

## Configuration

### **Variables**

| Variable                   | Description                   | Default       |
| -------------------------- | ----------------------------- | ------------- |
| `aws_region`               | AWS region for AMI build      | `us-west-2`   |
| `instance_type`            | Instance type for build       | `c7a.4xlarge` |
| `boundless_bento_version`  | Bento version to install      | `1.0.1`       |
| `boundless_broker_version` | Broker version to install     | `v1.0.0`      |
| `service_account_ids`      | Account IDs to share AMI with | `[]`          |
| `environment`              | Environment name for tagging  | `production`  |

### **Environment-Specific Configuration**

```bash
# Staging environment
packer build \
  -var "environment=staging" \
  -var "boundless_bento_version=v1.0.1" \
  -var "boundless_broker_version=v1.0.0" \
  -var "service_account_ids=[\"123456789012\"]" \
  bento.pkr.hcl

# Production environment
packer build \
  -var "environment=production" \
  -var "boundless_bento_version=v1.0.1" \
  -var "boundless_broker_version=v1.0.0" \
  -var "service_account_ids=[\"123456789012\",\"987654321098\"]" \
  bento.pkr.hcl

# GPU-optimized for prover nodes
packer build \
  -var "instance_type=g6.xlarge" \
  -var "boundless_bento_version=v1.0.1" \
  -var "boundless_broker_version=v1.0.0" \
  bento.pkr.hcl
```

## AMI Sharing

### **Automatic Sharing**

- AMIs are automatically shared with service accounts after build
- Lambda function handles cross-account sharing
- EventBridge triggers sharing on successful build

### **Manual Sharing**

```bash
# Share AMI with specific accounts
aws ec2 modify-image-attribute \
  --image-id ami-1234567890abcdef0 \
  --launch-permission Add='{UserId=123456789012}' \
  --region us-west-2
```

## AMI Contents

### **Pre-installed Components**

- **Bento Services**: API, Broker, Executor, Aux, Prover
- **NVIDIA Drivers**: CUDA 13.0 toolkit
- **RISC0**: Groth16 prover support
- **Vector**: Log forwarding to CloudWatch
- **AWS Tools**: CLI v2, SSM Agent, CloudWatch Agent

### **Logging Configuration**

- **Vector**: Configured to forward logs to CloudWatch
- **Destination**: `/boundless/bento` log group
- **Streams**: Daily rotation with format `%Y-%m-%d`
- **Services**: Captures logs from `bento.service` and `bento-broker.service`

## Troubleshooting

### **Common Issues**

1. **Build Failures**:
   ```bash
   # Check Packer logs
   export PACKER_LOG=1
   export PACKER_LOG_PATH=packer.log
   packer build bento.pkr.hcl
   ```

2. **AMI Sharing Issues**:
   ```bash
   # Check Lambda logs
   aws logs describe-log-groups --log-group-name-prefix "/aws/lambda/packer-ami-sharing"
   ```

3. **Permission Issues**:
   ```bash
   # Check IAM roles and policies
   aws iam get-role --role-name packer-build-role
   aws iam list-attached-role-policies --role-name packer-build-role
   ```

## Best Practices

1. **Version Control**: Always specify version numbers for Bento and Broker
2. **Testing**: Test AMIs in staging before production
3. **Cleanup**: Regularly clean up old AMIs to save costs
4. **Documentation**: Keep README and configurations up to date

## Security

### **AMI Security**

- **Encryption**: AMIs encrypted at rest
- **Sharing**: Only shared with authorized accounts
- **Tags**: Proper tagging for compliance and tracking

## Contributing

When making changes:

1. Test locally first
2. Update documentation
3. Use meaningful commit messages
4. Test in staging environment
5. Create pull requests for review
