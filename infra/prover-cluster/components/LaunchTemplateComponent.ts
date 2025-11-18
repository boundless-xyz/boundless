import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface LaunchTemplateConfig extends BaseComponentConfig {
  imageId: pulumi.Output<string>;
  instanceType: string;
  securityGroupId: pulumi.Output<string>;
  iamInstanceProfileName: pulumi.Output<string>;
  managerIp?: pulumi.Output<string>;
  taskDBName: string;
  taskDBUsername: string;
  taskDBPassword: string;
  ethRpcUrl?: pulumi.Output<string>;
  privateKey?: pulumi.Output<string>;
  orderStreamUrl?: string;
  verifierAddress?: string;
  boundlessMarketAddress?: string;
  setVerifierAddress?: string;
  collateralTokenAddress?: string;
  chainId?: string;
  componentType: "manager" | "prover" | "execution" | "aux";
  volumeSize?: number;
  rdsEndpoint?: pulumi.Output<string>;
  redisEndpoint?: pulumi.Output<string>;
  s3BucketName?: pulumi.Output<string>;
  s3AccessKeyId?: pulumi.Output<string>;
  s3SecretAccessKey?: pulumi.Output<string>;
  // Broker configuration
  mcyclePrice?: string;
  peakProveKhz?: number;
  minDeadline?: number;
  lookbackBlocks?: number;
  maxCollateral?: string;
  maxFileSize?: string;
  maxMcycleLimit?: string;
  maxConcurrentProofs?: number;
  balanceWarnThreshold?: string;
  balanceErrorThreshold?: string;
  collateralBalanceWarnThreshold?: string;
  collateralBalanceErrorThreshold?: string;
  priorityRequestorAddresses?: string;
  denyRequestorAddresses?: string;
  maxFetchRetries?: number;
  allowClientAddresses?: string;
  lockinPriorityGas?: string;
}

export class LaunchTemplateComponent extends BaseComponent {
  public readonly launchTemplate: aws.ec2.LaunchTemplate;

  constructor(config: LaunchTemplateConfig) {
    super(config, "boundless-bento");
    this.launchTemplate = this.createLaunchTemplate(config);
  }

  private createLaunchTemplate(config: LaunchTemplateConfig): aws.ec2.LaunchTemplate {
    const userData = this.generateUserData(config);

    return new aws.ec2.LaunchTemplate(`${config.componentType}-launch-template`, {
      name: this.generateName(`${config.componentType}-template`),
      imageId: config.imageId,
      instanceType: config.instanceType,
      vpcSecurityGroupIds: [config.securityGroupId],
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
          Name: this.generateTagName(config.componentType),
          Type: config.componentType,
          Environment: this.config.environment,
          Project: "boundless-bento-cluster",
          InstanceType: config.instanceType,
          "ssm:bootstrap": config.componentType,
        },
      }],
    });
  }

  private generateUserData(config: LaunchTemplateConfig): pulumi.Output<string> {
    if (config.componentType === "manager") {
      return this.generateManagerUserData(config);
    } else {
      return this.generateWorkerUserData(config);
    }
  }
  private generateManagerUserData(config: LaunchTemplateConfig): pulumi.Output<string> {
    return pulumi.all([
      config.taskDBName,
      config.taskDBUsername,
      config.taskDBPassword,
      config.ethRpcUrl!,
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
      config.redisEndpoint!,
      config.s3BucketName!,
      config.s3AccessKeyId!,
      config.s3SecretAccessKey!,
      config.mcyclePrice || "0.00000001",
      config.peakProveKhz || 100,
      config.minDeadline || 0,
      config.lookbackBlocks || 0,
      config.maxCollateral || "200",
      config.maxFileSize || "0",
      config.maxMcycleLimit || "0",
      config.maxConcurrentProofs || 1,
      config.balanceWarnThreshold || "0",
      config.balanceErrorThreshold || "0",
      config.collateralBalanceWarnThreshold || "0",
      config.collateralBalanceErrorThreshold || "0",
      config.priorityRequestorAddresses || "",
      config.denyRequestorAddresses || "",
      config.maxFetchRetries || 3,
      config.allowClientAddresses || "",
      config.lockinPriorityGas || "0",
    ]).apply(([dbName, dbUser, dbPass, rpcUrl, privKey, orderStreamUrl, verifierAddress, boundlessMarketAddress, setVerifierAddress, collateralTokenAddress, chainId, stackName, componentType, rdsEndpoint, redisEndpoint, s3BucketName, s3AccessKeyId, s3SecretAccessKey, mcyclePrice, peakProveKhz, minDeadline, lookbackBlocks, maxCollateral, maxFileSize, maxMcycleLimit, maxConcurrentProofs, balanceWarnThreshold, balanceErrorThreshold, collateralBalanceWarnThreshold, collateralBalanceErrorThreshold, priorityRequestorAddresses, denyRequestorAddresses, maxFetchRetries, allowClientAddresses, lockinPriorityGas]) => {
      // Extract host from endpoints (format: host:port)
      const rdsEndpointStr = String(rdsEndpoint);
      const redisEndpointStr = String(redisEndpoint);
      const rdsHost = rdsEndpointStr.split(':')[0];
      const rdsPort = rdsEndpointStr.split(':')[1] || '5432';
      const redisHost = redisEndpointStr.split(':')[0];
      const redisPort = redisEndpointStr.split(':')[1] || '6379';

      const brokerTomlContent = `[market]
mcycle_price = "${mcyclePrice}"
mcycle_price_collateral_token = "0"
peak_prove_khz = ${peakProveKhz}
min_deadline = ${minDeadline}
lookback_blocks = ${lookbackBlocks}
max_collateral = "${maxCollateral}"
max_file_size = "${maxFileSize}"
max_mcycle_limit = "${maxMcycleLimit}"
max_concurrent_proofs = ${maxConcurrentProofs}
balance_warn_threshold = "${balanceWarnThreshold}"
balance_error_threshold = "${balanceErrorThreshold}"
collateral_balance_warn_threshold = "${collateralBalanceWarnThreshold}"
collateral_balance_error_threshold = "${collateralBalanceErrorThreshold}"
priority_requestor_addresses = "${priorityRequestorAddresses}"
deny_requestor_addresses = "${denyRequestorAddresses}"
max_fetch_retries = ${maxFetchRetries}
allow_client_addresses = "${allowClientAddresses}"
lockin_priority_gas = "${lockinPriorityGas}"

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
          aggregation_dimensions: [["InstanceId"]]
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

  - path: /etc/environment.d/bento.conf
    content: |
      RUST_LOG=info
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
      PROVER_RPC_URL=${rpcUrl}
      PROVER_PRIVATE_KEY=${privKey}
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
    cat /etc/environment.d/bento.conf >> /etc/environment
  - |
    /usr/bin/sed -i 's|group_name: "/boundless/bent.*"|group_name: "/boundless/bento/${stackName}/${componentType}"|g' /etc/vector/vector.yaml
  - |
    /usr/bin/sed -i 's|"namespace": "Boundless/Services/bent.*",|"namespace": "Boundless/Services/${stackName}/bento-${componentType}",|g' /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json
  - |
    cp /opt/boundless/config/bento-api.service /etc/systemd/system/bento-api.service
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
    systemctl start bento-api.service bento-broker.service
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
      config.redisEndpoint!,
      config.s3BucketName!,
      config.s3AccessKeyId!,
      config.s3SecretAccessKey!
    ]).apply(([managerIp, dbName, dbUser, dbPass, stackName, componentType, rdsEndpoint, redisEndpoint, s3BucketName, s3AccessKeyId, s3SecretAccessKey]) => {
      // Extract host from endpoints (format: host:port)
      const rdsEndpointStr = String(rdsEndpoint);
      const redisEndpointStr = String(redisEndpoint);
      const rdsHost = rdsEndpointStr.split(':')[0];
      const rdsPort = rdsEndpointStr.split(':')[1] || '5432';
      const redisHost = redisEndpointStr.split(':')[0];
      const redisPort = redisEndpointStr.split(':')[1] || '6379';
      const commonEnvVars = this.generateCommonEnvVars(managerIp, dbName, dbUser, dbPass, stackName, componentType, rdsHost, rdsPort, redisHost, redisPort, s3BucketName, s3AccessKeyId, s3SecretAccessKey);

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
  ): string {
    return `# Database and Redis URLs (AWS services)
echo "DATABASE_URL=postgresql://${dbUser}:${dbPass}@${rdsHost}:${rdsPort}/${dbName}" >> /etc/environment
echo "REDIS_URL=redis://${redisHost}:${redisPort}" >> /etc/environment

# S3 Configuration - using AWS S3
echo "RUST_LOG=info" >> /etc/environment
echo "S3_BUCKET=${s3BucketName}" >> /etc/environment
echo "S3_URL=https://s3.us-west-2.amazonaws.com" >> /etc/environment
echo "S3_ACCESS_KEY=${s3AccessKeyId}" >> /etc/environment
echo "S3_SECRET_KEY=${s3SecretAccessKey}" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "REDIS_TTL=57600" >> /etc/environment
echo "STACK_NAME=${stackName}" >> /etc/environment
echo "COMPONENT_TYPE=${componentType}" >> /etc/environment
/usr/bin/sed -i 's|group_name: "/boundless/bent.*"|group_name: "/boundless/bento/${stackName}/${componentType}"|g' /etc/vector/vector.yaml
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
