import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";
import * as inputs from "@pulumi/aws/types/input";

export interface LaunchTemplateConfig extends BaseComponentConfig {
  imageId: pulumi.Output<string>;
  instanceType: string;
  securityGroupId: pulumi.Output<string>;
  iamInstanceProfileName: pulumi.Output<string>;
  managerIp?: pulumi.Output<string>;
  taskDBName: string;
  taskDBUsername: string;
  taskDBPassword: string;
  privateKey?: pulumi.Output<string>;
  orderStreamUrl?: pulumi.Output<string>;
  verifierAddress?: string;
  boundlessMarketAddress?: string;
  setVerifierAddress?: string;
  collateralTokenAddress?: string;
  chainId?: string;
  componentType: "manager" | "prover" | "execution" | "aux";
  volumeSize?: number;
  networkInterfaceId?: pulumi.Output<string>;
  rdsEndpoint?: pulumi.Output<string>;
  s3BucketName?: pulumi.Output<string>;
  s3AccessKeyId?: pulumi.Output<string>;
  s3SecretAccessKey?: pulumi.Output<string>;
  // Broker configuration
  brokerRpcUrls?: pulumi.Output<string>;
  mcyclePrice?: string;
  peakProveKhz?: number;
  minDeadline?: number;
  lookbackBlocks?: number;
  maxCollateral?: string;
  maxFileSize?: string;
  maxMcycleLimit?: string;
  maxConcurrentProofs?: number;
  maxConcurrentPreflights?: number;
  maxJournalBytes?: number;
  balanceWarnThreshold?: string;
  balanceErrorThreshold?: string;
  collateralBalanceWarnThreshold?: string;
  collateralBalanceErrorThreshold?: string;
  priorityRequestorAddresses?: string;
  denyRequestorAddresses?: string;
  maxFetchRetries?: number;
  allowRequestorLists?: string;
  lockinPriorityGas?: string;
  orderCommitmentPriority?: string;
  rustLogLevel?: string;
}

export class LaunchTemplateComponent extends BaseComponent {
  public readonly launchTemplate: aws.ec2.LaunchTemplate;

  constructor(config: LaunchTemplateConfig) {
    super(config, "boundless-bento");
    this.launchTemplate = this.createLaunchTemplate(config);
  }

  private createLaunchTemplate(config: LaunchTemplateConfig): aws.ec2.LaunchTemplate {
    const userData = this.generateUserData(config);

    let networkingConfig
    if (config.networkInterfaceId) {
      // If a network interface ID is provided, use it
      networkingConfig = {
        networkInterfaces: this.generateNetworkInterfaceConfig(config)
      }
    } else {
      // Without a network interface ID, must specify security group IDs
      networkingConfig = {
        vpcSecurityGroupIds: [config.securityGroupId]
      }
    }

    return new aws.ec2.LaunchTemplate(`${config.componentType}-launch-template`, {
      name: this.generateName(`${config.componentType}-template`),
      imageId: config.imageId,
      instanceType: config.instanceType,
      iamInstanceProfile: {
        name: config.iamInstanceProfileName,
      },
      userData: userData,
      blockDeviceMappings: [{
        deviceName: "/dev/sda1",
        ebs: {
          volumeSize: config.volumeSize || 100,
          volumeType: "gp3",
          deleteOnTermination: "true",
        },
      }],
      updateDefaultVersion: true,
      tagSpecifications: [{
        resourceType: "instance",
        tags: {
          Name: this.generateName(config.componentType),
          Type: config.componentType,
          Environment: this.config.environment,
          Project: "boundless-bento-cluster",
          InstanceType: config.instanceType,
          "ssm:bootstrap": config.componentType,
        },
      }],
      ...networkingConfig
    });
  }

  private generateUserData(config: LaunchTemplateConfig): pulumi.Output<string> {
    if (config.componentType === "manager") {
      return this.generateManagerUserData(config);
    } else {
      return this.generateWorkerUserData(config);
    }
  }

  private generateNetworkInterfaceConfig(config: LaunchTemplateConfig): Partial<inputs.ec2.LaunchTemplateNetworkInterface>[] {
    // Providing subnets, security groups, or public IP allocation policy is not allowed when
    // also providing a network interface ID. That configuration must be on the network interface
    return [{
      deleteOnTermination: "false",
      networkInterfaceId: config.networkInterfaceId,
      deviceIndex: 0
    }]
  }

  private generateManagerUserData(config: LaunchTemplateConfig): pulumi.Output<string> {
    return pulumi.all([
      config.taskDBName,
      config.taskDBUsername,
      config.taskDBPassword,
      config.privateKey!,
      config.orderStreamUrl!,
      config.verifierAddress!,
      config.boundlessMarketAddress!,
      config.setVerifierAddress!,
      config.collateralTokenAddress!,
      config.chainId!,
      this.config.stackName,
      config.componentType,
      config.rdsEndpoint!,
      config.s3BucketName!,
      config.s3AccessKeyId!,
      config.s3SecretAccessKey!,
      config.brokerRpcUrls!,
      config.mcyclePrice || "0.00000001",
      config.peakProveKhz || 100,
      config.minDeadline || 0,
      config.lookbackBlocks || 0,
      config.maxCollateral || "200",
      config.maxFileSize || "500000000000",
      config.maxMcycleLimit || "1000000000000",
      config.maxConcurrentProofs || 1,
      config.maxConcurrentPreflights || 2,
      config.maxJournalBytes || 20000,
      config.balanceWarnThreshold || "50",
      config.balanceErrorThreshold || "100",
      config.collateralBalanceWarnThreshold || "50",
      config.collateralBalanceErrorThreshold || "100",
      config.maxFetchRetries || 3,
      config.allowRequestorLists || "",
      config.lockinPriorityGas || "0",
      config.orderCommitmentPriority || "cycle_price",
      config.rustLogLevel || "debug",
    ]).apply(([dbName, dbUser, dbPass, privKey, orderStreamUrl, verifierAddress, boundlessMarketAddress, setVerifierAddress, collateralTokenAddress, chainId, stackName, componentType, rdsEndpoint, s3BucketName, s3AccessKeyId, s3SecretAccessKey, brokerRpcUrls, mcyclePrice, peakProveKhz, minDeadline, lookbackBlocks, maxCollateral, maxFileSize, maxMcycleLimit, maxConcurrentProofs, maxConcurrentPreflights, maxJournalBytes, balanceWarnThreshold, balanceErrorThreshold, collateralBalanceWarnThreshold, collateralBalanceErrorThreshold, maxFetchRetries, allowRequestorLists, lockinPriorityGas, orderCommitmentPriority, rustLogLevel]) => {
      const brokerRpcUrlsStr = brokerRpcUrls;
      // Extract host from endpoints (format: host:port)
      const rdsEndpointStr = String(rdsEndpoint);
      const rdsHost = rdsEndpointStr.split(':')[0];
      const rdsPort = rdsEndpointStr.split(':')[1] || '5432';
      // Manager runs Redis/Valkey locally, so use localhost
      const redisHost = "127.0.0.1";
      const redisPort = "6379";

      const brokerTomlContent = `[market]
mcycle_price = "${mcyclePrice}"
mcycle_price_collateral_token = "0"
peak_prove_khz = ${peakProveKhz}
min_deadline = ${minDeadline}
lookback_blocks = ${lookbackBlocks}
max_collateral = "${maxCollateral}"
max_file_size = ${maxFileSize}
max_mcycle_limit = ${maxMcycleLimit}
max_concurrent_proofs = ${maxConcurrentProofs}
max_journal_bytes = ${maxJournalBytes}
balance_warn_threshold = "${balanceWarnThreshold}"
balance_error_threshold = "${balanceErrorThreshold}"
collateral_balance_warn_threshold = "${collateralBalanceWarnThreshold}"
collateral_balance_error_threshold = "${collateralBalanceErrorThreshold}"
max_fetch_retries = ${maxFetchRetries}
${allowRequestorLists ? `allowed_requestor_lists = ${allowRequestorLists}\n` : ''}
${lockinPriorityGas ? `lockin_priority_gas = ${lockinPriorityGas}\n` : ''}
order_commitment_priority = "${orderCommitmentPriority}"
priority_requestor_lists = [
	"https://requestors.boundless.network/boundless-recommended-priority-list.standard.json",
]
max_concurrent_preflights = ${Number(maxConcurrentPreflights) - Number(maxConcurrentProofs)}

[prover]
status_poll_retry_count = 3
status_poll_ms = 1000
req_retry_count = 3
req_retry_sleep_ms = 500
proof_retry_count = 1
proof_retry_sleep_ms = 500

[batcher]
batch_max_time = 1000
min_batch_size = 1
block_deadline_buffer_secs = 120
txn_timeout = 10
single_txn_fulfill = true
max_submission_attempts = 2
withdraw = true
`;

      const aggregationDimensionsJson = JSON.stringify({
        metrics: {
          aggregation_dimensions: [["AutoScalingGroupName"]]
        }
      }, null, 2);

      const userDataScript = `#cloud-config
write_files:
  - path: /opt/boundless/broker.toml
    content: |
${brokerTomlContent.split('\n').map(line => `      ${line}`).join('\n')}
    owner: ubuntu:ubuntu
    permissions: '0644'

  - path: /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.d/aggregation_dimensions.json
    content: |
${aggregationDimensionsJson.split('\n').map(line => `      ${line}`).join('\n')}
    owner: root:root
    permissions: '0644'

  - path: /etc/systemd/system/bento-api.service
    content: |
      [Unit]
      Description=Boundless Bento API Service
      After=network.target

      [Service]
      Type=simple
      User=ubuntu
      EnvironmentFile=/etc/environment
      WorkingDirectory=/opt/boundless
      ExecStart=/usr/local/bin/api --bind-addr 0.0.0.0:8081
      Restart=always
      RestartSec=10
      TimeoutStartSec=1800
      TimeoutStopSec=300

      [Install]
      WantedBy=multi-user.target

  - path: /etc/environment.d/bento.conf
    content: |
      RUST_LOG=${rustLogLevel}
      BENTO_API_LISTEN_ADDR=0.0.0.0
      BENTO_API_PORT=8081
      SNARK_TIMEOUT=1800
      BENTO_BROKER_LISTEN_ADDR=0.0.0.0
      BENTO_BROKER_PORT=8082
      BENTO_EXECUTOR_LISTEN_ADDR=0.0.0.0
      BENTO_EXECUTOR_PORT=8083
      BENTO_PROVER_LISTEN_ADDR=0.0.0.0
      BENTO_PROVER_PORT=8086
      DATABASE_URL=postgresql://${dbUser}:${dbPass}@${rdsHost}:${rdsPort}/${dbName}
      REDIS_URL=redis://${redisHost}:${redisPort}
      S3_BUCKET=${s3BucketName}
      S3_URL=https://s3.us-west-2.amazonaws.com
      S3_ACCESS_KEY=${s3AccessKeyId}
      S3_SECRET_KEY=${s3SecretAccessKey}
      AWS_REGION=us-west-2
      STACK_NAME=${stackName}
      COMPONENT_TYPE=${componentType}
      PROVER_RPC_URLS=${brokerRpcUrlsStr}
      PROVER_PRIVATE_KEY=${privKey}
      PRIVATE_KEY=${privKey}
      ORDER_STREAM_URL=${orderStreamUrl}
      VERIFIER_ADDRESS=${verifierAddress}
      BOUNDLESS_MARKET_ADDRESS=${boundlessMarketAddress}
      SET_VERIFIER_ADDRESS=${setVerifierAddress}
      COLLATERAL_TOKEN_ADDRESS=${collateralTokenAddress}
      CHAIN_ID=${chainId}
    owner: root:root
    permissions: '0644'

runcmd:
  - |
    # Install Valkey (Redis fork) as a systemd service
    apt-get update
    apt-get install -y software-properties-common
    add-apt-repository -y ppa:valkey/valkey
    apt-get update
    apt-get install -y valkey-server
  - |
    # Configure Valkey to listen on all interfaces (for worker nodes to connect)
    cat > /etc/valkey/valkey.conf << 'VALKEYEOF'
    bind 0.0.0.0
    port 6379
    protected-mode no
    maxmemory 12gb
    maxmemory-policy allkeys-lru
    save ""
    VALKEYEOF
  - |
    # Enable and start Valkey service
    systemctl enable valkey-server
    systemctl restart valkey-server
  - |
    # Allow Redis/Valkey connections from worker nodes in the security group
    # (Security group rules are managed by Pulumi, but we ensure the service is ready)
    cat /etc/environment.d/bento.conf >> /etc/environment
  - |
    /usr/bin/sed -i 's|group_name: "/boundless/bent.*"|group_name: "/boundless/bento/${stackName}/${componentType}"|g' /etc/vector/vector.yaml
  - |
    echo "VECTOR_LOG=debug" >> /etc/default/vector
  - |
    /usr/bin/sed -i 's|"namespace": "Boundless/Services/bent.*",|"namespace": "Boundless/Services/${stackName}/bento-${componentType}",|g' /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json
  - |
    cp /opt/boundless/config/bento.service /etc/systemd/system/bento.service
    cp /opt/boundless/config/bento-broker.service /etc/systemd/system/bento-broker.service
  - |
    chown -R ubuntu:ubuntu /opt/boundless
  - |
    systemctl daemon-reload
    systemctl restart vector
    systemctl restart amazon-cloudwatch-agent
  - |
    # Reload environment and start services
    systemctl daemon-reload
    systemctl restart bento-api.service bento-broker.service
    systemctl enable bento-api.service bento-broker.service
`;
      // AWS Launch Templates require base64-encoded userdata (unlike EC2 Instances which accept plain text)
      return Buffer.from(userDataScript).toString('base64');
    });
  }

  private generateWorkerUserData(config: LaunchTemplateConfig): pulumi.Output<string> {
    return pulumi.all([
      config.managerIp!,
      config.taskDBName,
      config.taskDBUsername,
      config.taskDBPassword,
      this.config.stackName,
      config.componentType,
      config.rdsEndpoint!,
      config.s3BucketName!,
      config.s3AccessKeyId!,
      config.s3SecretAccessKey!,
      config.rustLogLevel || "debug",
    ]).apply(([managerIp, dbName, dbUser, dbPass, stackName, componentType, rdsEndpoint, s3BucketName, s3AccessKeyId, s3SecretAccessKey, rustLogLevel]) => {
      // Extract host from endpoints (format: host:port)
      const rdsEndpointStr = String(rdsEndpoint);
      const rdsHost = rdsEndpointStr.split(':')[0];
      const rdsPort = rdsEndpointStr.split(':')[1] || '5432';
      // Workers connect to Redis/Valkey on the manager node
      const redisHost = managerIp;
      const redisPort = "6379";
      const commonEnvVars = this.generateCommonEnvVars(managerIp, dbName, dbUser, dbPass, stackName, componentType, rdsHost, rdsPort, redisHost, redisPort, s3BucketName, s3AccessKeyId, s3SecretAccessKey, rustLogLevel);

      let componentSpecificVars = "";
      let serviceFile = "";

      switch (config.componentType) {
        case "prover":
          serviceFile = "bento-prover.service";
          componentSpecificVars = `
# Prover-specific variables and commands

# Add prover-specific CloudWatch agent configuration
cat > /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.d/gpu_metrics.json << 'EOF'
{
  "metrics": {
    "metrics_collected": {
      "nvidia_gpu": {
        "measurement": [
          "utilization_gpu",
          "memory_total",
          "memory_used",
          "memory_free",
          "power_draw",
          "temperature_gpu"
        ]
      }
    }
  }
}
EOF
`;
          break;
        case "execution":
          serviceFile = "bento-executor.service";
          componentSpecificVars = `
# Execution-specific environment variables
echo "FINALIZE_RETRIES=3" >> /etc/environment
echo "FINALIZE_TIMEOUT=60" >> /etc/environment
echo "SEGMENT_PO2=21" >> /etc/environment`;
          break;
        case "aux":
          serviceFile = "bento-aux.service";
          break;
      }

      const userDataScript = `#!/bin/bash
${commonEnvVars}
${componentSpecificVars}

# Copy and configure service file
cp /etc/systemd/system/${serviceFile} /etc/systemd/system/bento.service
systemctl daemon-reload
systemctl restart vector
systemctl restart amazon-cloudwatch-agent
systemctl start bento.service
systemctl enable bento.service`;

      // AWS Launch Templates require base64-encoded userdata (unlike EC2 Instances which accept plain text)
      return Buffer.from(userDataScript).toString('base64');
    });
  }

  private generateCommonEnvVars(
    managerIp: string,
    dbName: string,
    dbUser: string,
    dbPass: string,
    stackName: string,
    componentType: string,
    rdsHost: string,
    rdsPort: string,
    redisHost: string,
    redisPort: string,
    s3BucketName: string,
    s3AccessKeyId: string,
    s3SecretAccessKey: string,
    rustLogLevel: string,
  ): string {
    return `# Database and Redis URLs (AWS services)
echo "DATABASE_URL=postgresql://${dbUser}:${dbPass}@${rdsHost}:${rdsPort}/${dbName}" >> /etc/environment
echo "REDIS_URL=redis://${redisHost}:${redisPort}" >> /etc/environment

# S3 Configuration - using AWS S3
echo "RUST_LOG=${rustLogLevel}" >> /etc/environment
echo "S3_BUCKET=${s3BucketName}" >> /etc/environment
echo "S3_URL=https://s3.us-west-2.amazonaws.com" >> /etc/environment
echo "S3_ACCESS_KEY=${s3AccessKeyId}" >> /etc/environment
echo "S3_SECRET_KEY=${s3SecretAccessKey}" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "REDIS_TTL=57600" >> /etc/environment
echo "STACK_NAME=${stackName}" >> /etc/environment
echo "COMPONENT_TYPE=${componentType}" >> /etc/environment

/usr/bin/sed -i 's|group_name: "/boundless/bent.*"|group_name: "/boundless/bento/${stackName}/${componentType}"|g' /etc/vector/vector.yaml
echo "VECTOR_LOG=debug" >> /etc/default/vector

/usr/bin/sed -i 's|"namespace": "Boundless/Services/bent.*",|"namespace": "Boundless/Services/${stackName}/bento-${componentType}-cluster",|g' /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json

# Add CloudWatch agent configuration common to all worker clusters
cat > /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.d/aggregation_dimensions.json << 'EOF'
{
  "metrics": {
    "aggregation_dimensions": [["AutoScalingGroupName"]]
  }
}
EOF
`;
  }
}
