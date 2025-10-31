# Boundless Bento Prover Cluster

This directory contains the Pulumi infrastructure code for deploying a Boundless Bento prover cluster on AWS.

## Architecture Overview

### Compute Resources

- **Broker** (1x t3.medium EC2): Dedicated instance with SQLite database
- **Bento API** (AWS Fargate): ECS service providing REST API for Bento proving
- **Exec Agents** (1x r7iz.2xlarge): ECS with 4 exec agent containers
- **SNARK Agent** (1x c7a.4xlarge): ECS with 1 SNARK agent container
- **GPU Provers** (8x g6e.xlarge): ECS with 1 GPU prover container each
- **Aux Agent** (AWS Fargate): ECS service for monitoring and requeuing

### Data Services

- **PostgreSQL** (db.t4g.micro): Task database for ECS services
- **RDS Proxy**: Connection pooling for PostgreSQL
- **ElastiCache Redis** (cache.r7g.large): Caching layer
- **S3 Buckets**: Configuration and workflow storage
- **SQLite Database**: Local broker state

### Networking

- Custom VPC with public and private subnets
- Services deployed in private subnets
- Security groups for service isolation

## Directory Structure

```
infra/prover-cluster/
├── templates/            # Cloud-init template files
│   ├── manager.yaml      # Manager node cloud-init template
│   └── prover.yaml       # Prover node cloud-init template
├── index.ts              # Main Pulumi program
├── Pulumi.dev.yaml       # Pulumi configuration
├── deploy.sh             # Deployment script
└── README.md             # This file
```

## Template Files

The cloud-init configurations are now separated into template files for better readability and maintainability:

- **`templates/manager.yaml`**: Manager node cloud-init template with Pulumi variable interpolation
- **`templates/prover.yaml`**: Prover node cloud-init template with Pulumi variable interpolation

## Prerequisites

1. AWS CLI configured with appropriate credentials
2. Pulumi CLI installed
3. Node.js and npm installed
4. SSH key pair named `<stack_name>-keypair` in AWS
   ```bash
   aws ec2 create-key-pair --key-name <stack_name>-keypair --query "KeyMaterial" --output text > <stack_name>-keypair.pem
   chmod 400 <stack_name>-keypair.pem
   ```

## Deployment

### 1. Install Dependencies

```bash
cd infra/prover-cluster
npm install
```

### 2. Initialize Pulumi Stack

```bash
pulumi login --local
pulumi stack init <stack-name>
pulumi stack select <stack-name>
pulumi config set aws:region us-west-2
```

For dev deployments, either comment out the backend URL in `Pulumi.yaml`, or set the `PULUMI_BACKEND_URL` env var to
keep
the stack configuration local, for example with `PULUMI_BACKEND_URL="file://~"`.

### 3. Configure Required Values

```bash
# Basic configuration
pulumi config set environment custom
pulumi config set gitBranch main
pulumi config set segmentSize 21
pulumi config set snarkTimeout 180

# Required secrets (encrypted)
pulumi config set brokerPrivateKey <YOUR_PRIVATE_KEY> --secret
pulumi config set rpcUrl <YOUR_RPC_URL> --secret
pulumi config set rdsPassword <YOUR_PASSWORD> --secret
pulumi config set dockerToken '{"username":"<DOCKER_USERNAME>", "password":"<PERSONAL_ACCESS_TOKEN>"}' --secret
pulumi config set orderStreamUrl <ORDER_STREAM_URL> --secret

# Optional configuration
pulumi config set boundlessMarketAddress <YOUR_MARKET_CONTRACT_ADDRESS>
pulumi config set setVerifierAddress <YOUR_VERIFIER_CONTRACT_ADDRESS>
```

### 4. Deploy Infrastructure

```bash
# Preview changes
PULUMI_CONFIG_PASSPHRASE=<your-passphrase> pulumi preview

# Deploy
PULUMI_CONFIG_PASSPHRASE=<your-passphrase> pulumi up
```

### 5. Get Outputs

```bash
pulumi stack output                    # Get all outputs
pulumi stack output dashboardUrl       # Get dashboard URL
pulumi stack output bentoAPIUrl        # Get Bento API URL
pulumi stack output alertsTopicArn     # Get alerts topic
```

## Management

### Broker Service (EC2)

```bash
# Get broker instance ID
BROKER_ASG_NAME=$(pulumi stack output --json | jq -r '.default.brokerInstanceArn' | cut -d'/' -f2)
BROKER_INSTANCE_ID=$(aws autoscaling describe-auto-scaling-groups --auto-scaling-group-names $BROKER_ASG_NAME --query 'AutoScalingGroups[0].Instances[0].InstanceId' --output text)

# SSH access (requires bastion/VPN)
aws ssm start-session --target $BROKER_INSTANCE_ID

# Check logs from userdata start up
sudo cat /var/log/cloud-init-output.log

# Check service status
sudo systemctl status boundless-broker.service
sudo journalctl -u boundless-broker.service -f

# Restart service
sudo systemctl restart boundless-broker.service
```

### ECS Services

```bash
# Check service status
aws ecs describe-services --cluster prover-cluster --services prover-cluster-exec-agents-service
aws ecs describe-services --cluster prover-cluster --services prover-cluster-snark-agent-service
aws ecs describe-services --cluster prover-cluster --services prover-cluster-gpu-provers-service
aws ecs describe-services --cluster prover-cluster-aux-cluster --services prover-cluster-bento-api-service

# View logs
aws logs tail prover-cluster-exec-agents-logs --follow
aws logs tail prover-cluster-snark-agent-logs --follow
aws logs tail prover-cluster-gpu-provers-logs --follow
aws logs tail prover-cluster-bento-api-logs --follow
```

### Check Instance Status

```bash
# Check manager instance
aws ec2 describe-instances \
  --filters 'Name=tag:Project,Values=boundless-bento-cluster' 'Name=tag:Type,Values=manager' \
  --query 'Reservations[*].Instances[*].[InstanceId,State.Name,Tags[?Key==`Name`].Value|[0]]' \
  --output table

# Check prover instances
aws ec2 describe-instances \
  --filters 'Name=tag:Project,Values=boundless-bento-cluster' 'Name=tag:Type,Values=prover' \
  --query 'Reservations[*].Instances[*].[InstanceId,State.Name,Tags[?Key==`Name`].Value|[0]]' \
  --output table

# Check ASG status
aws autoscaling describe-auto-scaling-groups \
  --auto-scaling-group-names boundless-bento-prover-asg \
  --query 'AutoScalingGroups[0].[AutoScalingGroupName,DesiredCapacity,MinSize,MaxSize,Instances[].InstanceId]' \
  --output table
```

## Monitoring

### CloudWatch Dashboard

```bash
# Get dashboard URL
pulumi stack output dashboardUrl
```

### Alerts

```bash
# Subscribe to email notifications
aws sns subscribe --topic-arn $(pulumi stack output alertsTopicArn) --protocol email --notification-endpoint your-email@example.com
```

### Logs

```bash
# Broker logs
aws logs tail /aws/ec2/boundless-broker --follow

# ECS service logs
aws logs tail prover-cluster-exec-agents-logs --follow
aws logs tail prover-cluster-snark-agent-logs --follow
aws logs tail prover-cluster-gpu-provers-logs --follow
aws logs tail prover-cluster-bento-api-logs --follow
```

## Configuration Updates

### Update Broker Configuration

```bash
# Update SSM parameter
aws ssm put-parameter --name "/boundless/prover-cluster/broker-config" --value '{"segmentSize": 21, ...}' --type String --overwrite

# Update secrets
aws secretsmanager update-secret --secret-id prover-cluster-broker-private-key --secret-string "new-private-key"
aws secretsmanager update-secret --secret-id prover-cluster-rpc-url --secret-string "new-rpc-url"

# Restart broker
aws ssm send-command --instance-ids $BROKER_INSTANCE_ID --document-name "AWS-RunShellScript" --parameters 'commands=["sudo systemctl restart boundless-broker.service"]'
```

### Update Infrastructure

```bash
pulumi config set <key> <value>
PULUMI_CONFIG_PASSPHRASE=<your-passphrase> pulumi up
```

## Scaling

### ECS Services

```bash
# Scale services
aws ecs update-service --cluster prover-cluster --service prover-cluster-gpu-provers-service --desired-count 12
aws ecs update-service --cluster prover-cluster --service prover-cluster-exec-agents-service --desired-count 2
aws ecs update-service --cluster prover-cluster-aux-cluster --service prover-cluster-bento-api-service --desired-count 2
```

### Auto Scaling Groups

```bash
# Scale underlying instances
aws autoscaling update-auto-scaling-group --auto-scaling-group-name prover-cluster-gpu-asg --desired-capacity 12
aws autoscaling update-auto-scaling-group --auto-scaling-group-name prover-cluster-exec-asg --desired-capacity 2
```

## Benefits of Auto Scaling Group

1. **Automatic Scaling**: Provers scale up/down based on demand
2. **Health Checks**: Unhealthy instances are automatically replaced
3. **Fault Tolerance**: Failed instances are automatically recreated
4. **Cost Optimization**: Scale down during low usage periods
5. **Load Distribution**: Instances spread across multiple AZs
6. **Easy Management**: Single ASG manages all prover instances

## Benefits of Template Files

1. **Readability**: YAML files are easier to read and understand
2. **Maintainability**: Changes to cloud-init don't require touching TypeScript
3. **Reusability**: Template files can be reused or referenced
4. **Version Control**: Better diff tracking for configuration changes
5. **Validation**: YAML syntax highlighting and validation in editors
6. **Pulumi Integration**: Direct variable interpolation with `${variable}` syntax

## Troubleshooting

### Common Issues

1. **Broker service not starting**
   ```bash
   sudo journalctl -u boundless-broker.service -n 100
   sudo /opt/boundless/setup-env.sh
   ```

2. **ECS services not starting**
   ```bash
   aws logs tail prover-cluster-<component>-logs --since 1h
   aws ecs describe-services --cluster prover-cluster --services prover-cluster-<component>-service
   ```

3. **Database connection errors**
   ```bash
   psql $DATABASE_URL -c "SELECT 1"
   ```

4. **Service Failures**: Check logs with `journalctl -u <service-name>`
5. **Database Connection**: Ensure PostgreSQL is listening on `0.0.0.0`
6. **SSM Access**: Verify security group allows outbound HTTPS traffic

### Debug Commands

```bash
# Check broker instance
aws ssm send-command --instance-ids $BROKER_INSTANCE_ID --document-name "AWS-RunShellScript" --parameters 'commands=["df -h"]'

# Check service status
aws ecs describe-services --cluster prover-cluster --services prover-cluster-exec-agents-service
aws rds describe-db-instances --db-instance-identifier prover-cluster-postgres
aws elasticache describe-cache-clusters --cache-cluster-id prover-cluster-redis
```

### Logs

```bash
# Service logs
journalctl -u bento-aux.service -f

# Cloud-init logs
tail -f /var/log/cloud-init-output.log

# Broker logs
aws logs tail /aws/ec2/boundless-broker --follow

# ECS service logs
aws logs tail prover-cluster-exec-agents-logs --follow
aws logs tail prover-cluster-snark-agent-logs --follow
aws logs tail prover-cluster-gpu-provers-logs --follow
aws logs tail prover-cluster-bento-api-logs --follow
```

## Backup

### Broker Database

```bash
# Backup SQLite database
aws ssm send-command --instance-ids $BROKER_INSTANCE_ID --document-name "AWS-RunShellScript" --parameters 'commands=["sudo cp /opt/boundless/data/broker.db /tmp/broker-backup-$(date +%Y%m%d).db"]'
```

## Cleanup

```bash
# Destroy infrastructure
PULUMI_CONFIG_PASSPHRASE=<your-passphrase> pulumi destroy

# Remove stack
pulumi stack rm <stack-name>
```

## Configuration Reference

| Key                      | Description                    | Required |
| ------------------------ | ------------------------------ | -------- |
| `environment`            | Environment name               | Yes      |
| `gitBranch`              | Git branch to use              | Yes      |
| `segmentSize`            | Bento segment size             | Yes      |
| `snarkTimeout`           | SNARK timeout in seconds       | Yes      |
| `brokerPrivateKey`       | Broker private key (secret)    | Yes      |
| `rpcUrl`                 | Ethereum RPC endpoint (secret) | Yes      |
| `boundlessMarketAddress` | Market contract address        | No       |
| `setVerifierAddress`     | Verifier contract address      | No       |

## Instance Types

| Component   | Instance Type | Count | vCPUs | Memory |
| ----------- | ------------- | ----- | ----- | ------ |
| Broker      | t3.medium     | 1     | 2     | 4 GB   |
| Bento API   | Fargate       | 1     | 1     | 2 GB   |
| Exec Agents | r7iz.2xlarge  | 1     | 8     | 64 GB  |
| SNARK Agent | c7a.4xlarge   | 1     | 16    | 32 GB  |
| GPU Provers | g6e.xlarge    | 8     | 4     | 16 GB  |
| Aux Agent   | Fargate       | 1     | 1     | 2 GB   |

## Outputs

| Output                  | Description                   |
| ----------------------- | ----------------------------- |
| `brokerInstanceArn`     | Broker Auto Scaling Group ARN |
| `bentoAPIServiceArn`    | Bento API ECS service ARN     |
| `bentoAPIUrl`           | Bento API internal URL        |
| `ecsClusterArn`         | ECS cluster ARN               |
| `databaseEndpoint`      | RDS database endpoint         |
| `databaseProxyEndpoint` | RDS proxy endpoint            |
| `redisEndpoint`         | ElastiCache Redis endpoint    |
| `dashboardUrl`          | CloudWatch dashboard URL      |
| `alertsTopicArn`        | SNS topic ARN for alerts      |
| `vpcId`                 | VPC ID                        |
