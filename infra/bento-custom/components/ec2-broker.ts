import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as fs from "fs";
import * as path from "path";

export async function setupEC2Broker(
    name: string,
    network: any,
    storage: any,
    secrets: any,
    tags: Record<string, string>,
    bentoAPIUrl: pulumi.Output<string>
) {
    const config = new pulumi.Config();

    // Get current AWS account ID
    const current = await aws.getCallerIdentity();

    // Configuration
    const brokerConfig = {
        segmentSize: config.getNumber("segmentSize") || 21,
        snarkTimeout: config.getNumber("snarkTimeout") || 180,
        setVerifierAddress: config.get("setVerifierAddress") || "",
        boundlessMarketAddress: config.get("boundlessMarketAddress") || "",
        gitBranch: config.get("gitBranch") || "main",
        environment: config.get("environment") || "custom",
        region: config.get("aws:region") || "us-west-2",
        accountId: current.accountId,
    };

    // Create IAM role for EC2 broker
    const brokerRole = new aws.iam.Role(`${name}-broker-role`, {
        assumeRolePolicy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [{
                Action: "sts:AssumeRole",
                Effect: "Allow",
                Principal: {
                    Service: "ec2.amazonaws.com"
                }
            }]
        }),
        tags: {
            ...tags,
            Name: `${name}-broker-role`,
        },
    });

    // Attach necessary policies
    new aws.iam.RolePolicyAttachment(`${name}-broker-ssm-policy`, {
        role: brokerRole.name,
        policyArn: "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
    });

    new aws.iam.RolePolicyAttachment(`${name}-broker-cloudwatch-policy`, {
        role: brokerRole.name,
        policyArn: "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
    });

    // Custom policy for accessing secrets and S3
    const brokerCustomPolicy = new aws.iam.Policy(`${name}-broker-custom-policy`, {
        policy: pulumi.all([
            storage.configBucket.arn
        ]).apply(([bucketArn]) =>
            JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "secretsmanager:GetSecretValue",
                            "secretsmanager:DescribeSecret"
                        ],
                        Resource: `arn:aws:secretsmanager:${brokerConfig.region}:${brokerConfig.accountId}:secret:${name}/*`
                    },
                    {
                        Effect: "Allow",
                        Action: [
                            "s3:GetObject",
                            "s3:GetObjectVersion",
                            "s3:ListBucket"
                        ],
                        Resource: [
                            bucketArn,
                            `${bucketArn}/*`
                        ]
                    },
                    {
                        Effect: "Allow",
                        Action: [
                            "ssm:GetParameter",
                            "ssm:GetParameters",
                            "ssm:GetParametersByPath"
                        ],
                        Resource: `arn:aws:ssm:${brokerConfig.region}:${brokerConfig.accountId}:parameter/boundless/${name}/*`
                    }
                ]
            })
        ),
        tags: {
            ...tags,
            Name: `${name}-broker-custom-policy`,
        },
    });

    new aws.iam.RolePolicyAttachment(`${name}-broker-custom-policy-attachment`, {
        role: brokerRole.name,
        policyArn: brokerCustomPolicy.arn,
    });

    // Create instance profile
    const brokerInstanceProfile = new aws.iam.InstanceProfile(`${name}-broker-instance-profile`, {
        role: brokerRole.name,
        tags: {
            ...tags,
            Name: `${name}-broker-instance-profile`,
        },
    });

    // Create SSM parameter for broker configuration
    const brokerConfigParam = new aws.ssm.Parameter(`${name}-broker-config`, {
        name: `/boundless/${name}/broker-config`,
        type: "String",
        value: JSON.stringify({
            name,
            region: brokerConfig.region,
            bucketName: storage.configBucket.id,
            setVerifierAddress: brokerConfig.setVerifierAddress,
            boundlessMarketAddress: brokerConfig.boundlessMarketAddress,
            gitBranch: brokerConfig.gitBranch,
            segmentSize: brokerConfig.segmentSize,
            snarkTimeout: brokerConfig.snarkTimeout,
            environment: brokerConfig.environment,
            secretArns: {
                privateKey: secrets.brokerPrivateKey.arn,
                rpcUrl: secrets.rpcUrl.arn,
            },
            logJson: true
        }),
        tags: {
            ...tags,
            Name: `${name}-broker-config`,
        },
    });

    // Use the shared broker security group from network setup

    // User data script to set up the broker
    const userData = pulumi.all([
        brokerConfigParam.name,
        storage.configBucket.id,
        brokerConfig.region,
        bentoAPIUrl
    ]).apply(([configParamName, bucketName, region, bentoApiUrl]) => {
        const userDataScript = `#!/bin/bash
set -e

# Update system
apt-get update -y
apt-get install -y awscli jq docker.io docker-compose-plugin

# Install cloudwatch agent
wget https://s3.amazonaws.com/amazoncloudwatch-agent/ubuntu/amd64/latest/amazon-cloudwatch-agent.deb
dpkg -i amazon-cloudwatch-agent.deb
rm amazon-cloudwatch-agent.deb

# Install Rust for building the broker
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# Start and enable docker
systemctl start docker
systemctl enable docker
usermod -aG docker ubuntu

# Create directory for broker data
mkdir -p /opt/boundless/data
chown -R ubuntu:ubuntu /opt/boundless

# Create script to fetch configuration and secrets
cat > /opt/boundless/setup-env.sh << 'EOF'
#!/bin/bash
set -e

# Fetch configuration from SSM
CONFIG_JSON=$(aws ssm get-parameter --name "${configParamName}" --region ${region} --query 'Parameter.Value' --output text)
export BROKER_CONFIG="$CONFIG_JSON"

# Parse configuration
export PRIVATE_KEY_ARN=$(echo $CONFIG_JSON | jq -r '.secretArns.privateKey')
export RPC_URL_ARN=$(echo $CONFIG_JSON | jq -r '.secretArns.rpcUrl')
export GIT_BRANCH=$(echo $CONFIG_JSON | jq -r '.gitBranch')
export SEGMENT_SIZE=$(echo $CONFIG_JSON | jq -r '.segmentSize')
export SNARK_TIMEOUT=$(echo $CONFIG_JSON | jq -r '.snarkTimeout')
export BOUNDLESS_MARKET_ADDRESS=$(echo $CONFIG_JSON | jq -r '.boundlessMarketAddress')
export SET_VERIFIER_ADDRESS=$(echo $CONFIG_JSON | jq -r '.setVerifierAddress')

# Fetch secrets
export PRIVATE_KEY=$(aws secretsmanager get-secret-value --secret-id "$PRIVATE_KEY_ARN" --region ${region} --query 'SecretString' --output text)
export RPC_URL=$(aws secretsmanager get-secret-value --secret-id "$RPC_URL_ARN" --region ${region} --query 'SecretString' --output text)
export ORDER_STREAM_URL=$(aws secretsmanager get-secret-value --secret-id "$ORDER_STREAM_URL_ARN" --region ${region} --query 'SecretString' --output text)

# Create environment file for broker
cat > /opt/boundless/.env.broker << EOL
PRIVATE_KEY=$PRIVATE_KEY
RPC_URL=$RPC_URL
ORDER_STREAM_URL=$ORDER_STREAM_URL
DATABASE_URL=sqlite:///opt/boundless/data/broker.db
BENTO_API_URL=${bentoApiUrl}
RUST_LOG=info
RUST_BACKTRACE=1
BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS
SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS
SEGMENT_SIZE=$SEGMENT_SIZE
SNARK_TIMEOUT=$SNARK_TIMEOUT
EOL

chmod 600 /opt/boundless/.env.broker
chown ubuntu:ubuntu /opt/boundless/.env.broker
EOF

chmod +x /opt/boundless/setup-env.sh
chown ubuntu:ubuntu /opt/boundless/setup-env.sh

# Clone the Boundless repository
cd /opt/boundless
git clone https://github.com/boundless-xyz/boundless.git repo
cd repo
git checkout ${brokerConfig.gitBranch}
chown -R ubuntu:ubuntu /opt/boundless

# Build the broker binary
cd /opt/boundless/repo
RISC0_SKIP_BUILD=1 cargo build --release --bin broker
chown -R ubuntu:ubuntu /opt/boundless

# Copy default broker configuration
cp broker-template.toml /opt/boundless/broker.toml
chown ubuntu:ubuntu /opt/boundless/broker.toml

# Create systemd service for broker
cat > /etc/systemd/system/boundless-broker.service << 'EOF'
[Unit]
Description=Boundless Broker Service
After=network.target docker.service
Requires=docker.service

[Service]
Type=simple
User=ubuntu
WorkingDirectory=/opt/boundless/repo
ExecStartPre=/opt/boundless/setup-env.sh
ExecStart=/opt/boundless/repo/target/release/broker --db-url sqlite:///opt/boundless/data/broker.db --config-file /opt/boundless/broker.toml --bento-api-url ${bentoApiUrl}
ExecStop=/bin/kill -TERM $MAINPID
Restart=always
RestartSec=10
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
EOF

# Enable and start the service
systemctl daemon-reload
systemctl enable boundless-broker.service

# Setup CloudWatch agent configuration
cat > /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json << 'EOF'
{
    "agent": {
        "metrics_collection_interval": 60,
        "run_as_user": "ubuntu"
    },
    "metrics": {
        "namespace": "Boundless/Broker",
        "metrics_collected": {
            "cpu": {
                "measurement": [
                    "cpu_usage_idle",
                    "cpu_usage_iowait",
                    "cpu_usage_user",
                    "cpu_usage_system"
                ],
                "metrics_collection_interval": 60,
                "resources": [
                    "*"
                ],
                "totalcpu": false
            },
            "disk": {
                "measurement": [
                    "used_percent"
                ],
                "metrics_collection_interval": 60,
                "resources": [
                    "*"
                ]
            },
            "mem": {
                "measurement": [
                    "mem_used_percent"
                ],
                "metrics_collection_interval": 60
            }
        }
    },
    "logs": {
        "logs_collected": {
            "files": {
                "collect_list": [
                    {
                        "file_path": "/var/log/messages",
                        "log_group_name": "/aws/ec2/${name}-broker",
                        "log_stream_name": "{instance_id}/messages"
                    }
                ]
            }
        }
    }
}
EOF

# Start CloudWatch agent
systemctl start amazon-cloudwatch-agent
systemctl enable amazon-cloudwatch-agent

# Start the broker service
systemctl start boundless-broker.service

# Signal success
/opt/aws/bin/cfn-signal -e $? --stack ${name} --resource BrokerInstance --region ${region}
`;

        return Buffer.from(userDataScript).toString('base64');
    });

    // Create launch template for broker
    const brokerLaunchTemplate = new aws.ec2.LaunchTemplate(`${name}-broker-launch-template`, {
        namePrefix: `${name}-broker-`,
        imageId: "ami-0897831b586e1015f", // Amazon Linux 2023 in us-west-2
        instanceType: "t3.medium", // Sufficient for broker with SQLite
        keyName: `${name}-keypair`, // Make sure this key exists

        vpcSecurityGroupIds: [network.brokerSecurityGroup.id],

        iamInstanceProfile: {
            name: brokerInstanceProfile.name,
        },

        userData: userData,

        blockDeviceMappings: [{
            deviceName: "/dev/sda1",
            ebs: {
                volumeSize: 50,
                volumeType: "gp3",
                encrypted: "true",
                deleteOnTermination: "true",
            },
        }],

        tagSpecifications: [
            {
                resourceType: "instance",
                tags: {
                    ...tags,
                    Name: `${name}-broker`,
                },
            },
            {
                resourceType: "volume",
                tags: {
                    ...tags,
                    Name: `${name}-broker-volume`,
                },
            },
        ],

        tags: {
            ...tags,
            Name: `${name}-broker-launch-template`,
        },
    });

    // Create Auto Scaling Group for broker (single instance)
    const brokerASG = new aws.autoscaling.Group(`${name}-broker-asg`, {
        name: `${name}-broker-asg`,
        vpcZoneIdentifiers: network.vpc.privateSubnetIds,

        launchTemplate: {
            id: brokerLaunchTemplate.id,
            version: "$Latest",
        },

        minSize: 1,
        maxSize: 1,
        desiredCapacity: 1,

        healthCheckType: "EC2",
        healthCheckGracePeriod: 300,

        tags: [
            {
                key: "Name",
                value: `${name}-broker-asg`,
                propagateAtLaunch: false,
            },
            ...Object.entries(tags).map(([key, value]) => ({
                key,
                value,
                propagateAtLaunch: true,
            })),
        ],
    });

    // Create CloudWatch log group for broker
    const brokerLogGroup = new aws.cloudwatch.LogGroup(`${name}-broker-logs`, {
        name: `/aws/ec2/${name}-broker`,
        retentionInDays: 7,
        tags: {
            ...tags,
            Name: `${name}-broker-logs`,
        },
    });

    return {
        instance: brokerASG,
        launchTemplate: brokerLaunchTemplate,
        securityGroup: network.brokerSecurityGroup,
        logGroup: brokerLogGroup,
        role: brokerRole,
        config: brokerConfig,
    };
}