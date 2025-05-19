import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import { config } from 'process';
import { getEnvVar, ChainId, getServiceNameV1, Severity } from "../../util";
import { createProverAlarms } from './brokerAlarms';

export class BentoEC2Broker extends pulumi.ComponentResource {
    public updateCommandArn: pulumi.Output<string>;
    public updateCommandId: pulumi.Output<string>;
    public updateInstanceRole: aws.iam.Role;
    public instance: aws.ec2.Instance;

    constructor(name: string, args: {
        chainId: string;
        gitBranch: string;
        segmentSize: number;
        privateKey: string | pulumi.Output<string>;
        ethRpcUrl: string | pulumi.Output<string>;
        orderStreamUrl: string | pulumi.Output<string>;
        baseStackName: string;
        vpcId: pulumi.Output<any>;
        pubSubNetIds: pulumi.Output<any>;
        dockerDir: string;
        dockerTag: string;
        setVerifierAddress: string;
        boundlessMarketAddress: string;
        ciCacheSecret?: pulumi.Output<string>;
        githubTokenSecret?: pulumi.Output<string>;
        brokerTomlPath: string;
        boundlessAlertsTopicArn?: string;
        sshPublicKey?: string | pulumi.Output<string>;
    }, opts?: pulumi.ComponentResourceOptions) {
        super(name, name, opts);

        const { boundlessMarketAddress, setVerifierAddress, ethRpcUrl, sshPublicKey, brokerTomlPath, privateKey, orderStreamUrl, pubSubNetIds, gitBranch, boundlessAlertsTopicArn, segmentSize } = args;

        const region = "us-west-2";
        const serviceName = name;

        let sshKey: aws.ec2.KeyPair | undefined = undefined;
        if (sshPublicKey) {
            sshKey = new aws.ec2.KeyPair("ssh-key", {
                publicKey: sshPublicKey,
            });
        }

        const ethRpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerEthRpc`);
        new aws.secretsmanager.SecretVersion(`${serviceName}-brokerEthRpc`, {
            secretId: ethRpcUrlSecret.id,
            secretString: ethRpcUrl,
        });

        const privateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-brokerPrivateKey`);
        new aws.secretsmanager.SecretVersion(`${serviceName}-privateKeyValue`, {
            secretId: privateKeySecret.id,
            secretString: privateKey,
        });

        const orderStreamUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerOrderStreamUrl`);
        new aws.secretsmanager.SecretVersion(`${serviceName}-brokerOrderStreamUrl`, {
            secretId: orderStreamUrlSecret.id,
            secretString: orderStreamUrl,
        });

        const brokerS3Bucket = new aws.s3.Bucket(serviceName, {
            bucketPrefix: serviceName,
            tags: {
                Name: serviceName,
            },
        });
        const brokerS3BucketName = brokerS3Bucket.bucket.apply(n => n);

        const tomlToUpload = new pulumi.asset.FileAsset(brokerTomlPath);
        const setupScriptToUpload = new pulumi.asset.FileAsset("../../scripts/setup.sh");
        const justfileToUpload = new pulumi.asset.FileAsset("../../justfile");
        const composeYmlToUpload = new pulumi.asset.FileAsset("../../compose.yml");
        const brokerTomlBucketObject = new aws.s3.BucketObject(serviceName, {
            bucket: brokerS3Bucket.id,
            key: 'broker.toml',
            source: tomlToUpload,
        });

        const setupScriptBucketObject = new aws.s3.BucketObject(`${serviceName}-setup-script`, {
            bucket: brokerS3Bucket.id,
            key: 'setup.sh',
            source: setupScriptToUpload,
        });

        const justfileBucketObject = new aws.s3.BucketObject(`${serviceName}-justfile`, {
            bucket: brokerS3Bucket.id,
            key: 'justfile',
            source: justfileToUpload,
        });

        const composeYmlBucketObject = new aws.s3.BucketObject(`${serviceName}-compose-yml`, {
            bucket: brokerS3Bucket.id,
            key: 'compose.yml',
            source: composeYmlToUpload,
        });

        // Create security group for the EC2 instance
        const securityGroup = new aws.ec2.SecurityGroup(`${name}-sg`, {
            vpcId: args.vpcId,
            description: "Enable SSH access and outbound access",
            ingress: [
                {
                    protocol: "tcp",
                    fromPort: 22,
                    toPort: 22,
                    cidrBlocks: ["0.0.0.0/0"],
                },
            ],
            egress: [
                {
                    protocol: "-1",
                    fromPort: 0,
                    toPort: 0,
                    cidrBlocks: ["0.0.0.0/0"],
                },
            ],
            tags: {
                Name: `${name}-sg`,
            },
        }, { parent: this });

        // Create IAM role for the EC2 instance
        const role = new aws.iam.Role(`${name}-role`, {
            assumeRolePolicy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [{
                    Action: "sts:AssumeRole",
                    Principal: {
                        Service: "ec2.amazonaws.com",
                    },
                    Effect: "Allow",
                }],
            }),
        }, { parent: this });

        // Attach policies to enable us to use SSM to run updates on the instance
        new aws.iam.RolePolicyAttachment(`${name}-ssm-policy`, {
            role: role.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
        }, { parent: this });

        // Add policy to allow access to secrets, get config files from S3, get config from SSM, and write logs to CloudWatch
        new aws.iam.RolePolicy(`${name}-policy`, {
            role: role.id,
            policy: {
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "secretsmanager:GetSecretValue",
                            "secretsmanager:ListSecrets"
                        ],
                        Resource: [
                            ethRpcUrlSecret.arn,
                            privateKeySecret.arn,
                            orderStreamUrlSecret.arn
                        ],
                    },
                    {
                        Effect: 'Allow',
                        Action: [
                            's3:GetObject',
                            's3:ListObject',
                            's3:HeadObject'
                        ],
                        Resource: [
                            brokerTomlBucketObject.arn,
                            setupScriptBucketObject.arn,
                            justfileBucketObject.arn,
                            composeYmlBucketObject.arn
                        ],
                    },
                    {
                        Effect: "Allow",
                        Action: [
                            "ssm:SendCommand",
                            "ssm:GetParameter",
                            "ssm:GetParameters"
                        ],
                        Resource: [
                            pulumi.interpolate`arn:aws:ssm:*:*:parameter/*`
                        ]
                    },
                    {
                        Effect: 'Allow',
                        Action: [
                            'logs:CreateLogGroup',
                            'logs:CreateLogStream',
                            'logs:PutLogEvents',
                            'logs:DescribeLogStreams'
                        ],
                        Resource: [
                            'arn:aws:logs:*:*:log-group:*'
                        ]
                    },
                    {
                        Effect: 'Allow',
                        Action: [
                            'ec2:DescribeInstanceStatus',
                            'ec2:DescribeTags',
                            'ec2:DescribeVolumes',
                            'ec2:DescribeTags'
                        ],
                        Resource: '*'
                    },
                    {
                        Effect: 'Allow',
                        Action: [
                            'cloudwatch:PutMetricData',
                        ],
                        Resource: '*'
                    }
                ]
            }
        }, { parent: this });

        // Create SSM document for updates
        const updateDocument = new aws.ssm.Document(`${name}-update-doc`, {
            name: `${name}-update-doc`,
            documentType: "Command",
            content: JSON.stringify({
                schemaVersion: "2.2",
                description: "Update Bento Prover",
                mainSteps: [
                    {
                        action: "aws:runShellScript",
                        name: "updateProver",
                        inputs: {
                            runCommand: [
                                `set -eu`,
                                `echo "Fetching Git Branch and Broker Config"`,
                                `export CONFIG=$(aws --region ${region} ssm get-parameter --name "/boundless/${name}/broker-config" --query Parameter.Value --output text)`,
                                `export BUCKET=$(echo $CONFIG | jq -r '.brokerBucket')`,
                                `export GIT_BRANCH=$(echo $CONFIG | jq -r '.gitBranch')`,
                                `echo "Stopping Broker"`,
                                "systemctl stop boundless-broker.service",
                                `echo "Updating source code to latest on $GIT_BRANCH"`,
                                "git reset --hard",
                                `git checkout $GIT_BRANCH`,
                                `git pull`,
                                `echo "Copying Broker Toml"`,
                                "aws s3 cp s3://$BUCKET/broker.toml ./broker.toml",
                                `echo "Copying Justfile"`,
                                "aws s3 cp s3://$BUCKET/justfile ./justfile",
                                `echo "Copying Compose Yml"`,
                                "aws s3 cp s3://$BUCKET/compose.yml ./compose.yml",
                                `echo "Refreshing Broker Env File"`,
                                "/local/create-broker-env.sh",
                                `echo "Restarting Broker"`,
                                "systemctl start boundless-broker.service"
                            ],
                            timeoutSeconds: 3600,
                            workingDirectory: "/local/boundless"
                        }
                    }
                ]
            }),
        }, { parent: this });

        // Create IAM role for CI/CD to execute SSM commands
        const updateInstanceRole = new aws.iam.Role(`${name}-update-instance-role`, {
            assumeRolePolicy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [{
                    Action: "sts:AssumeRole",
                    Principal: {
                        Service: "codebuild.amazonaws.com",
                    },
                    Effect: "Allow",
                }],
            }),
        }, { parent: this });

        // Attach policy to allow CI role to execute SSM commands
        new aws.iam.RolePolicy(`${name}-update-instance-ssm-policy`, {
            role: updateInstanceRole.id,
            policy: {
                Version: "2012-10-17",
                Statement: [{
                    Effect: "Allow",
                    Action: [
                        "ssm:SendCommand",
                        "ssm:GetParameter",
                        "ssm:GetParameters"
                    ],
                    Resource: [
                        updateDocument.arn,
                        pulumi.interpolate`arn:aws:ssm:*:*:parameter/*`
                    ]
                },
                {
                    Effect: 'Allow',
                    Action: [
                        's3:GetObject',
                        's3:ListObject',
                        's3:HeadObject'
                    ],
                    Resource: [
                        brokerTomlBucketObject.arn
                    ],
                }]
            }
        }, { parent: this });

        // Store all config in a json string in SSM. 
        // EC2 instances will fetch this config and use it to setup the environment.
        // During updates, the instance will fetch the latest config from SSM and overwrite the existing files.
        const ethRpcUrlSecretArn = ethRpcUrlSecret.arn.apply(arn => arn);
        const privateKeySecretArn = privateKeySecret.arn.apply(arn => arn);
        const orderStreamUrlSecretArn = orderStreamUrlSecret.arn.apply(arn => arn);
        const brokerConfigName = `/boundless/${name}/broker-config`;
        pulumi.all([
            brokerS3BucketName,
            ethRpcUrlSecretArn,
            privateKeySecretArn,
            orderStreamUrlSecretArn
        ]).apply(([brokerBucket, ethRpcArn, privateKeyArn, orderStreamArn]) => {
            new aws.ssm.Parameter(`${name}-broker-config`, {
                name: brokerConfigName,
                type: "String",
                value: JSON.stringify({
                    name,
                    region,
                    brokerBucket,
                    setVerifierAddress: setVerifierAddress,
                    boundlessMarketAddress,
                    gitBranch,
                    segmentSize,
                    secretArns: {
                        ethRpcUrl: ethRpcArn,
                        privateKey: privateKeyArn,
                        orderStreamUrl: orderStreamArn
                    }
                }),
            }, { parent: this });
        });

        // User data script to set up the Boundless Prover
        const userData = pulumi.interpolate`#!/bin/bash
set -euxo pipefail

# Format and mount the instance store volume. 
# We use instance storage rather than EBS to avoid issues with sqlite.
export VOLUME_NAME=/dev/$(lsblk -o NAME,MODEL,SIZE,MOUNTPOINT | awk '/NVMe Instance Storage/ {print $1}' | head -n1)
mkfs -t xfs $VOLUME_NAME
mkdir -p /local
mount $VOLUME_NAME /local

# Add mount information to fstab using UUID for stability across reboots
UUID=$(blkid -s UUID -o value "$VOLUME_NAME")
if ! grep -q "$UUID" /etc/fstab; then
    echo "UUID=$UUID /local xfs defaults,nofail 0 2" >> /etc/fstab
fi

# Ensure ubuntu user has access to /local directory and all future files/folders
# This user is used by systemd to run the broker.
chown ubuntu:ubuntu /local
chmod 775 /local
apt-get install -y acl
setfacl -R -d -m u::rwx,g::rwx,o::rx /local
setfacl -R -m u::rwx,g::rwx,o::rx /local

# Install Just, jq, AWS CLI
apt-get update
apt-get install -y awscli jq
snap install --edge --classic just

# Install CloudWatch agent.
curl -O https://s3.amazonaws.com/amazoncloudwatch-agent/ubuntu/amd64/latest/amazon-cloudwatch-agent.deb
sudo dpkg -i -E ./amazon-cloudwatch-agent.deb


# Create CloudWatch agent configuration
cat > /opt/aws/amazon-cloudwatch-agent/bin/config.json << 'EOF'
{
    "agent": {
        "metrics_collection_interval": 60,
        "run_as_user": "root"
    },
    "logs": {
        "logs_collected": {
            "files": {
                "collect_list": [
                    {
                        "file_path": "/var/log/setup.log",
                        "log_group_name": "${name}/setup-script-logs",
                        "log_stream_name": "${name}/setup-script-logs"
                    },
                    {
                        "file_path": "/var/log/syslog",
                        "log_group_name": "${name}/syslog",
                        "log_stream_name": "${name}/syslog"
                    },
                    {
                        "file_path": "/var/log/*.log",
                        "log_group_name": "${name}/all-var-logs",
                        "log_stream_name": "${name}/all-var-logs"
                    },
                    {
                        "file_path": "/local/docker/containers/*/*.log",
                        "log_group_name": "${name}",
                        "log_stream_name": "${name}"
                    }
                ]
            }
        }
    }
}
EOF

# Copy config to default location. Restart CloudWatch agent.
sudo cp /opt/aws/amazon-cloudwatch-agent/bin/config.json \
   /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.json

systemctl daemon-reload
systemctl enable amazon-cloudwatch-agent
systemctl start amazon-cloudwatch-agent

# Get config from Parameter Store needed for setup.
export CONFIG=$(aws --region ${region} ssm get-parameter --name ${brokerConfigName} --query Parameter.Value --output text)

# Clone Boundless repository into our instance storage
export GIT_BRANCH=$(echo $CONFIG | jq -r '.gitBranch')
cd /local
git clone https://github.com/boundless-xyz/boundless.git
cd boundless
git checkout $GIT_BRANCH

# Get the custom compose.yml, broker toml, justfile, and setup script that we uploaded to S3 in advance.
# This is only necessary when you are trying to deploy .
export BUCKET=$(echo $CONFIG | jq -r '.brokerBucket')
aws s3 cp s3://$BUCKET/compose.yml /local/boundless/compose.yml
aws s3 cp s3://$BUCKET/broker.toml /local/boundless/broker.toml
aws s3 cp s3://$BUCKET/setup.sh /local/boundless/scripts/setup.sh
aws s3 cp s3://$BUCKET/justfile /local/boundless/justfile

# Run Boundless setup scripts to install dependencies (docker, nvidia-container-toolkit, etc).
export SUDO_USER=ubuntu
export HOME=/home/ubuntu
chmod +x /local/boundless/scripts/setup.sh
/local/boundless/scripts/setup.sh

# Install nvcc. After reboot we will set the NVCC flags in the compose.yml using this.
sudo apt install nvidia-cuda-toolkit -y

# Create a script that
# 1/ creates a broker env file with all environment variables set.
# 2/ sets the NVCC flags in the compose.yml
# This script is run when the instance is rebooted, and when the instance is updated.
cat > /local/create-broker-env.sh << 'EOF'
#!/bin/bash
set -x

# Fetch and parse configuration from Parameter Store
CONFIG=$(aws --region ${region} ssm get-parameter --name ${brokerConfigName} --query Parameter.Value --output text)
REGION=$(echo $CONFIG | jq -r '.region')
BUCKET=$(echo $CONFIG | jq -r '.brokerBucket')
SET_VERIFIER_ADDRESS=$(echo $CONFIG | jq -r '.setVerifierAddress')
BOUNDLESS_MARKET_ADDRESS=$(echo $CONFIG | jq -r '.boundlessMarketAddress')
ETH_RPC_URL_SECRET_ARN=$(echo $CONFIG | jq -r '.secretArns.ethRpcUrl')
PRIVATE_KEY_SECRET_ARN=$(echo $CONFIG | jq -r '.secretArns.privateKey')
ORDER_STREAM_URL_SECRET_ARN=$(echo $CONFIG | jq -r '.secretArns.orderStreamUrl')
SEGMENT_SIZE=$(echo $CONFIG | jq -r '.segmentSize')

# Get secrets from AWS Secrets Manager
RPC_URL=$(aws --region ${region} secretsmanager get-secret-value --secret-id $ETH_RPC_URL_SECRET_ARN --query SecretString --output text)
PRIVATE_KEY=$(aws --region ${region} secretsmanager get-secret-value --secret-id $PRIVATE_KEY_SECRET_ARN --query SecretString --output text)
ORDER_STREAM_URL=$(aws --region ${region} secretsmanager get-secret-value --secret-id $ORDER_STREAM_URL_SECRET_ARN --query SecretString --output text)

# Create the env file for the broker.
rm -f /local/.env.broker
touch /local/.env.broker
chmod 770 /local/.env.broker

cat > /local/.env.broker << ENVEOF
HOME=/home/ubuntu
BUCKET=$BUCKET
SET_VERIFIER_ADDRESS=$SET_VERIFIER_ADDRESS
BOUNDLESS_MARKET_ADDRESS=$BOUNDLESS_MARKET_ADDRESS
AWS_REGION=$REGION
RPC_URL=$RPC_URL
PRIVATE_KEY=$PRIVATE_KEY
ORDER_STREAM_URL=$ORDER_STREAM_URL
SEGMENT_SIZE=$SEGMENT_SIZE
ENVEOF

EOF

chmod +x /local/create-broker-env.sh

# Update Docker daemon configuration to use the local instance store
jq '. + {"data-root": "/local/docker", "storage-driver": "overlay2"}' /etc/docker/daemon.json > /etc/docker/daemon.json.tmp
mv /etc/docker/daemon.json.tmp /etc/docker/daemon.json

# Restart Docker to apply changes
systemctl restart docker

# Create systemd service for broker that runs after the instance is rebooted.
cat > /etc/systemd/system/boundless-broker.service << 'EOF'
[Unit]
Description=Boundless Broker Service
After=network.target docker.service
Requires=docker.service
# Wait for the mount to be ready
After=local.mount
Requires=local.mount
# Wait for snapd to be ready
After=snapd.service
Requires=snapd.service

[Service]
Type=simple
User=ubuntu
ExecStartPre=/local/create-broker-env.sh
WorkingDirectory=/local/boundless
ExecStart=/snap/bin/just broker up /local/.env.broker detached=false
ExecStop=/snap/bin/just broker down
Restart=always
RestartSec=10
TimeoutStartSec=3600
TimeoutStopSec=120

[Install]
WantedBy=multi-user.target
EOF

# Enable and start the service
systemctl daemon-reload
systemctl enable boundless-broker.service

# Reboot the system to apply the changes from the setup script.
reboot
    `;

        const subnetId = pubSubNetIds.apply((subnets) => subnets[0]);
        const instanceProfile = new aws.iam.InstanceProfile(`${name}-profile`, {
            role: role.name,
        }, { parent: this });

        this.instance = new aws.ec2.Instance(`${name}-instance`, {
            instanceType: "g4dn.2xlarge", // 1 GPU, 1 x 225 NVMe SSD
            ami: "ami-016d360a89daa11ba", // Ubuntu 22.04 LTS amd64 AMI us-west-2
            subnetId: subnetId,
            keyName: sshKey?.keyName,
            vpcSecurityGroupIds: [securityGroup.id],
            iamInstanceProfile: instanceProfile.name,
            userData: userData,
            userDataReplaceOnChange: true,
            associatePublicIpAddress: true,
            tags: {
                Name: `${name}-instance`,
            },
            rootBlockDevice: {
                volumeSize: 100,
                volumeType: "gp3",
            },
        }, { parent: this });

        // Store the instance ID in SSM for retrieval during updates
        new aws.ssm.Parameter(`${name}-instance-id`, {
            name: `/boundless/${name}/instance-id`,
            type: "String",
            value: this.instance.id,
        }, { parent: this });

        // Ensure the log group exists for when we create the alarms.
        const logGroup = new aws.cloudwatch.LogGroup(`${name}-log-group`, {
            name: name,
            retentionInDays: 0,
        }, { parent: this });

        const alarmActions = boundlessAlertsTopicArn ? [boundlessAlertsTopicArn] : [];

        createProverAlarms(serviceName, logGroup, [logGroup, this.instance], alarmActions);

        this.updateCommandArn = updateDocument.arn;
        this.updateCommandId = updateDocument.id;
        this.updateInstanceRole = updateInstanceRole;
    }
}