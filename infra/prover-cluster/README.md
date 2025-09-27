# Boundless Bento Prover Cluster

This directory contains the Pulumi infrastructure code for deploying a Boundless Bento prover cluster on AWS.

## Architecture

The cluster consists of:

- **1 Manager Node**: Runs PostgreSQL, Redis, API, Executor, Aux, and Broker services
- **Auto Scaling Group of Prover Nodes**: Run the Bento prover agents (g6.xlarge instances) with automatic scaling

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

## Configuration

Edit `Pulumi.dev.yaml` to configure:

- Prover count (desired capacity)
- Min/max prover counts for auto scaling
- Database credentials
- Contract addresses
- And more...

## Deployment

1. **Prerequisites**:
   ```bash
   # Install Pulumi
   curl -fsSL https://get.pulumi.com | sh

   # Configure AWS credentials
   aws configure
   ```

2. **Deploy**:
   ```bash
   ./deploy.sh
   ```

   Or manually:
   ```bash
   pulumi up
   ```

## Management

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

### Connect to Instances

```bash
# Manager
aws ssm start-session --target <manager-instance-id>

# Prover (get instance ID from ASG)
aws autoscaling describe-auto-scaling-groups \
  --auto-scaling-group-names boundless-bento-prover-asg \
  --query 'AutoScalingGroups[0].Instances[0].InstanceId' \
  --output text | xargs -I {} aws ssm start-session --target {}
```

### Check Services

```bash
# On manager
sudo systemctl status boundless-api.service bento-executor.service bento-aux.service boundless-broker.service

# On prover
sudo systemctl status bento-prover.service
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

1. **Service Failures**: Check logs with `journalctl -u <service-name>`
2. **Database Connection**: Ensure PostgreSQL is listening on `0.0.0.0`
3. **SSM Access**: Verify security group allows outbound HTTPS traffic

### Logs

```bash
# Service logs
journalctl -u bento-aux.service -f

# Cloud-init logs
tail -f /var/log/cloud-init-output.log
```
