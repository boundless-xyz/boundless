# Boundless Packer AMI Builds

This directory contains Packer configurations for building Amazon Machine Images (AMIs) for the Boundless Bento infrastructure. The AMIs are built in the ops account and automatically shared with staging and production service accounts.

## Overview

The Packer setup provides:
- **Automated AMI builds**: Triggered by CI/CD pipelines
- **Cross-account sharing**: AMIs automatically shared with service accounts
- **Environment separation**: Different AMIs for different environments
- **GPU support**: NVIDIA CUDA and GPU drivers pre-installed
- **Service-ready**: All Bento services pre-configured

## Directory Structure

```
packer/
├── bento.pkr.hcl              # Main AMI build configuration
├── manager.pkr.hcl            # Manager node AMI (CPU-only)
├── prover.pkr.hcl             # Prover node AMI (GPU-enabled)
├── variables.pkr.hcl          # Variable definitions
├── service_files/             # Systemd service files
│   ├── bento-api.service
│   ├── bento-broker.service
│   ├── bento-executor.service
│   ├── bento-aux.service
│   ├── bento-prover.service
│   └── broker.toml
├── scripts/                   # Installation scripts
│   ├── setup.sh              # Main setup script
│   ├── setup-manager.sh      # Manager-specific setup
│   └── setup-prover.sh       # Prover-specific setup
└── README.md                  # This file
```

## AMI Types

### 1. **Manager AMI** (`manager.pkr.hcl`)
- **Purpose**: Manager nodes (PostgreSQL, Redis, API, Executor, Aux, Broker)
- **Instance Type**: CPU-optimized (c7a.4xlarge)
- **Services**: All Bento services except prover
- **GPU**: Not required

### 2. **Prover AMI** (`prover.pkr.hcl`)
- **Purpose**: Prover nodes (GPU-accelerated proof generation)
- **Instance Type**: GPU-optimized (g6.xlarge)
- **Services**: Bento prover service only
- **GPU**: NVIDIA CUDA 13.0, RISC0 Groth16

### 3. **Unified AMI** (`bento.pkr.hcl`)
- **Purpose**: General-purpose AMI for both manager and prover nodes
- **Instance Type**: Configurable
- **Services**: All Bento services
- **GPU**: Optional, based on instance type

## Pipeline Integration

### **Packer Pipeline** (`infra/pipelines/pipelines/packer.ts`)
- **Trigger**: Push to main branch
- **Build**: Creates AMI in ops account
- **Share**: Automatically shares AMI with staging and production accounts
- **Notify**: Sends notifications on success/failure

### **Prover Cluster Pipelines**
- **Staging**: `infra/pipelines/pipelines/prover-cluster-staging.ts`
- **Production**: `infra/pipelines/pipelines/prover-cluster-production.ts`
- **Trigger**: Manual or scheduled
- **Deploy**: Uses latest AMI to deploy prover clusters

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

   # Build manager AMI
   packer build -var "boundless_version=v1.0.1" manager.pkr.hcl

   # Build prover AMI
   packer build -var "boundless_version=v1.0.1" prover.pkr.hcl

   # Build unified AMI
   packer build -var "boundless_version=v1.0.1" bento.pkr.hcl
   ```

### **Pipeline Deployment**

The pipelines are automatically triggered and managed through the CI/CD system:

1. **AMI Build**: Triggered by code changes
2. **Cross-Account Sharing**: Automatic via Lambda function
3. **Cluster Deployment**: Manual or scheduled triggers

## Configuration

### **Variables**

| Variable | Description | Default |
|----------|-------------|---------|
| `aws_region` | AWS region for AMI build | `us-west-2` |
| `instance_type` | Instance type for build | `c7a.4xlarge` |
| `boundless_version` | Bento version to install | `latest` |
| `service_account_ids` | Account IDs to share AMI with | `[]` |
| `environment` | Environment name for tagging | `production` |

### **Environment-Specific Configuration**

```hcl
# Staging environment
packer build \
  -var "environment=staging" \
  -var "boundless_version=v1.0.1" \
  -var "service_account_ids=[\"123456789012\"]" \
  manager.pkr.hcl

# Production environment
packer build \
  -var "environment=production" \
  -var "boundless_version=v1.0.1" \
  -var "service_account_ids=[\"123456789012\",\"987654321098\"]" \
  manager.pkr.hcl
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

## Monitoring and Logging

### **CloudWatch Logs**
- Packer build logs: `/aws/codebuild/packer-build-project`
- AMI sharing logs: `/aws/lambda/packer-ami-sharing`
- Prover cluster logs: `/aws/codebuild/prover-cluster-*-deployment`

### **Notifications**
- **Slack**: Build status notifications
- **SNS**: Pipeline failure alerts
- **EventBridge**: AMI sharing events

## Troubleshooting

### **Common Issues**

1. **Build Failures**:
   ```bash
   # Check CodeBuild logs
   aws logs describe-log-groups --log-group-name-prefix "/aws/codebuild/packer"

   # View specific build logs
   aws logs get-log-events \
     --log-group-name "/aws/codebuild/packer-build-project" \
     --log-stream-name "build-id"
   ```

2. **AMI Sharing Issues**:
   ```bash
   # Check Lambda logs
   aws logs describe-log-groups --log-group-name-prefix "/aws/lambda/packer-ami-sharing"

   # Test Lambda function
   aws lambda invoke \
     --function-name packer-ami-sharing \
     --payload '{"detail":{"additional-information":{"environment":{"environment-variables":[{"name":"AMI_ID","value":"ami-1234567890abcdef0"}]}}}}' \
     response.json
   ```

3. **Permission Issues**:
   ```bash
   # Check IAM roles and policies
   aws iam get-role --role-name packer-build-role
   aws iam list-attached-role-policies --role-name packer-build-role
   ```

### **Debug Mode**

Enable debug logging for Packer builds:
```bash
export PACKER_LOG=1
export PACKER_LOG_PATH=packer.log
packer build manager.pkr.hcl
```

## Security

### **IAM Permissions**
- **Packer Role**: EC2, SSM, IAM permissions for AMI creation
- **Lambda Role**: EC2 permissions for AMI sharing
- **Cross-Account**: AssumeRole permissions for service accounts

### **AMI Security**
- **Encryption**: AMIs encrypted at rest
- **Sharing**: Only shared with authorized accounts
- **Tags**: Proper tagging for compliance and tracking

## Best Practices

1. **Version Control**: Always tag AMIs with version numbers
2. **Testing**: Test AMIs in staging before production
3. **Monitoring**: Monitor build times and success rates
4. **Cleanup**: Regularly clean up old AMIs to save costs
5. **Documentation**: Keep README and configurations up to date

## Cost Optimization

### **AMI Storage**
- **Lifecycle**: Automatically delete old AMIs
- **Compression**: Use efficient base images
- **Size**: Minimize AMI size where possible

### **Build Optimization**
- **Caching**: Use build caches for faster builds
- **Parallel**: Build multiple AMIs in parallel
- **Spot**: Use spot instances for build instances

## Support

For issues and questions:
1. Check the troubleshooting section above
2. Review CloudWatch logs
3. Check pipeline status in AWS Console
4. Contact the infrastructure team

## Contributing

When making changes:
1. Test locally first
2. Update documentation
3. Use meaningful commit messages
4. Test in staging environment
5. Create pull requests for review
