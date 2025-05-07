import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import { config } from 'process';
import { getEnvVar, ChainId, getServiceNameV1, Severity } from "../../util";
import { createProverAlarms } from './brokerAlarms';

export class BentoBroker extends pulumi.ComponentResource {
    public updateCommandArn: pulumi.Output<string>;
    public updateInstanceRole: aws.iam.Role;
    public instance: aws.ec2.Instance;

    constructor(name: string, args: {
        chainId: string;
        gitBranch: string;
        privateKey: string | pulumi.Output<string>;
        ethRpcUrl: string | pulumi.Output<string>;
        orderStreamUrl: string | pulumi.Output<string>;
        baseStackName: string;
        vpcId: pulumi.Output<any>;
        pubSubNetIds: pulumi.Output<any>;
        dockerDir: string;
        dockerTag: string;
        setVerifierAddr: string;
        boundlessMarketAddr: string;
        ciCacheSecret?: pulumi.Output<string>;
        githubTokenSecret?: pulumi.Output<string>;
        brokerTomlPath: string;
        boundlessAlertsTopicArn?: string;
        sshPublicKey?: string | pulumi.Output<string>;
    }, opts?: pulumi.ComponentResourceOptions) {
        super(name, name, opts);

        const { boundlessMarketAddr: proofMarketAddr, setVerifierAddr, ethRpcUrl, sshPublicKey, brokerTomlPath, privateKey, orderStreamUrl, pubSubNetIds, gitBranch, boundlessAlertsTopicArn } = args;

        const serviceName = name;

        // Generate an SSH key pair
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

        const fileToUpload = new pulumi.asset.FileAsset(brokerTomlPath);
        const setupScriptToUpload = new pulumi.asset.FileAsset("../../scripts/setup.sh");

        const bucketObject = new aws.s3.BucketObject(serviceName, {
            bucket: brokerS3Bucket.id,
            key: 'broker.toml',
            source: fileToUpload,
        });
        const setupScriptBucketObject = new aws.s3.BucketObject(`${serviceName}-setup-script`, {
            bucket: brokerS3Bucket.id,
            key: 'setup.sh',
            source: setupScriptToUpload,
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

        // Attach necessary policies to the role
        new aws.iam.RolePolicyAttachment(`${name}-ssm-policy`, {
            role: role.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
        }, { parent: this });

        // Add Secrets Manager policy to the role
        new aws.iam.RolePolicy(`${name}-secrets-policy`, {
            role: role.id,
            policy: {
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "secretsmanager:GetSecretValue"
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
                            bucketObject.arn,
                            setupScriptBucketObject.arn
                        ],
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
                            'arn:aws:logs:*:*:log-group:/*'
                        ]
                    }
                ]
            }
        }, { parent: this });

        // Create instance profile
        const instanceProfile = new aws.iam.InstanceProfile(`${name}-profile`, {
            role: role.name,
        }, { parent: this });

        // Create SSM document for updates
        const updateDocument = new aws.ssm.Document(`${name}-update-doc`, {
            name: `${name}-update-doc`,
            documentType: "Command",
            content: JSON.stringify({
                schemaVersion: "2.2",
                description: "Update Boundless Prover",
                mainSteps: [
                    {
                        action: "aws:runShellScript",
                        name: "updateProver",
                        inputs: {
                            runCommand: [
                                "cd /boundless",
                                "git fetch",
                                `git checkout ${gitBranch}`,
                                "git pull",
                                "just broker down",
                                `export ETH_RPC_URL=$(aws secretsmanager get-secret-value --secret-id ${ethRpcUrlSecret.arn} --query SecretString --output text)`,
                                `export PRIVATE_KEY=$(aws secretsmanager get-secret-value --secret-id ${privateKeySecret.arn} --query SecretString --output text)`,
                                `export ORDER_STREAM_URL=$(aws secretsmanager get-secret-value --secret-id ${orderStreamUrlSecret.arn} --query SecretString --output text)`,
                                `export BUCKET=${brokerS3BucketName}`,
                                `export SET_VERIFIER_ADDRESS=${setVerifierAddr}`,
                                `export BOUNDLESS_MARKET_ADDRESS=${proofMarketAddr}`,
                                "aws s3 cp s3://$BUCKET/broker.toml ./broker.toml",
                                "just broker"
                            ]
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
                        bucketObject.arn
                    ],
                }]
            }
        }, { parent: this });

        // User data script to set up the Boundless Prover
        const userData = pulumi.interpolate`#!/bin/bash
set -e

# Install AWS CLI and CloudWatch agent
apt-get update
apt-get install -y awscli

# Install Just
snap install --edge --classic just

# Install CloudWatch agent
wget https://s3.amazonaws.com/amazoncloudwatch-agent/ubuntu/amd64/latest/amazon-cloudwatch-agent.deb
dpkg -i amazon-cloudwatch-agent.deb

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
                        "file_path": "/var/log/syslog",
                        "log_group_name": "${name}/syslog",
                        "log_stream_name": "{instance_id}",
                        "timezone": "UTC"
                    },
                    {
                        "file_path": "/boundless/*.log",
                        "log_group_name": "${name}",
                        "log_stream_name": "{instance_id}",
                        "timezone": "UTC"
                    }
                ]
            }
        }
    }
}
EOF

# Start CloudWatch agent
/opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl -a fetch-config -m ec2 -s -c file:/opt/aws/amazon-cloudwatch-agent/bin/config.json
systemctl enable amazon-cloudwatch-agent
systemctl start amazon-cloudwatch-agent

# Get secrets from AWS Secrets Manager
export RPC_URL=$(aws secretsmanager get-secret-value --secret-id ${ethRpcUrlSecret.arn} --query SecretString --output text)
export PRIVATE_KEY=$(aws secretsmanager get-secret-value --secret-id ${privateKeySecret.arn} --query SecretString --output text)
export ORDER_STREAM_URL=$(aws secretsmanager get-secret-value --secret-id ${orderStreamUrlSecret.arn} --query SecretString --output text)
export BUCKET=${brokerS3BucketName}
export SET_VERIFIER_ADDRESS=${setVerifierAddr}
export BOUNDLESS_MARKET_ADDRESS=${proofMarketAddr}

export SUDO_USER=ubuntu
export HOME=/home/ubuntu

# Clone Boundless repository
git clone https://github.com/boundless-xyz/boundless.git
cd boundless
git checkout ${gitBranch}

aws s3 cp s3://$BUCKET/broker.toml ./broker.toml
aws s3 cp s3://$BUCKET/setup.sh ./scripts/setup.sh

# Run setup script
chmod +x ./scripts/setup.sh
./scripts/setup.sh

# Start the Boundless Prover with environment variables
cd /boundless
just broker
    `;

        // Create the EC2 instance
        const subnetId = pubSubNetIds.apply((subnets) => subnets[0]);
        this.instance = new aws.ec2.Instance(`${name}-instance`, {
            instanceType: "g4dn.xlarge",
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
                volumeSize: 200,
                volumeType: "gp3",
            },
        }, { parent: this });

        this.updateCommandArn = updateDocument.arn;
        this.updateInstanceRole = updateInstanceRole;

        // const alarmActions = boundlessAlertsTopicArn ? [boundlessAlertsTopicArn] : [];

        // createProverAlarms(serviceName, [instance], alarmActions);
    }
}