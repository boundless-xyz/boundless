import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { createInstanceProfile } from "./iam";

export async function setupEcsCluster(
    name: string,
    network: any,
    tags: Record<string, string>
) {
    // Create ECS cluster
    const cluster = new aws.ecs.Cluster(`${name}-cluster`, {
        name: `${name}-cluster`,
        settings: [{
            name: "containerInsights",
            value: "enabled",
        }],
        tags: {
            ...tags,
            Name: `${name}-cluster`,
        },
    });

    // Create instance profile for EC2 instances
    const instanceProfile = await createInstanceProfile(`${name}-ecs-instances`, tags);

    // Get standard AMI for CPU instances. Note AMI comes with ECS agent installed.
    const standardAmi = aws.ec2.getAmi({
        mostRecent: true,
        owners: ["amazon"],
        filters: [
            { name: "name", values: ["amzn2-ami-ecs-hvm-2.0.*-x86_64-ebs"] },
            { name: "virtualization-type", values: ["hvm"] },
        ],
    });

    // Get GPU-compatible AMI. Note AMI comes with ECS agent installed.
    const gpuAmi = aws.ec2.getAmi({
        mostRecent: true,
        owners: ["amazon"],
        filters: [
            { name: "name", values: ["amzn2-ami-ecs-gpu-hvm-2.0.*-x86_64-ebs"] },
            { name: "virtualization-type", values: ["hvm"] },
        ],
    });

    // Create launch template for exec agents (r7iz.2xlarge)
    const execLaunchTemplate = new aws.ec2.LaunchTemplate(`${name}-exec-launch-template`, {
        name: `${name}-exec-launch-template`,
        imageId: standardAmi.then(ami => ami.id),
        instanceType: "r7iz.2xlarge", // 8 vCPUs, 64 GB RAM, NVMe SSD

        iamInstanceProfile: {
            name: instanceProfile.name,
        },

        vpcSecurityGroupIds: [network.instanceSecurityGroup.id],

        blockDeviceMappings: [{
            deviceName: "/dev/xvda",
            ebs: {
                volumeSize: 100,
                volumeType: "gp3",
                encrypted: "true",
                deleteOnTermination: "true",
            },
        }],

        userData: cluster.name.apply(clusterName => Buffer.from(`#!/bin/bash
echo ECS_CLUSTER=${clusterName} >> /etc/ecs/ecs.config
echo ECS_ENABLE_CONTAINER_METADATA=true >> /etc/ecs/ecs.config
echo ECS_ENABLE_TASK_IAM_ROLE=true >> /etc/ecs/ecs.config
echo ECS_ENABLE_TASK_IAM_ROLE_NETWORK_HOST=true >> /etc/ecs/ecs.config
echo ECS_ENGINE_TASK_CLEANUP_WAIT_DURATION=10m >> /etc/ecs/ecs.config
echo ECS_CONTAINER_STOP_TIMEOUT=30s >> /etc/ecs/ecs.config
echo ECS_CONTAINER_START_TIMEOUT=3m >> /etc/ecs/ecs.config
echo ECS_DISABLE_PRIVILEGED=false >> /etc/ecs/ecs.config
echo ECS_BACKEND_HOST= >> /etc/ecs/ecs.config

yum install -y amazon-cloudwatch-agent
cat > /opt/aws/amazon-cloudwatch-agent/etc/amazon-cloudwatch-agent.d/ecs_default.json <<"EOL"
          {
            "logs": {
              "logs_collected": {
                "files": {
                  "collect_list": [
                    {
                      "file_path": "/var/log/ecs/ecs-init.log",
                      "log_group_name": "/${name}/ecs-init",
                      "log_stream_name": "{instance_id}"
                    },
                    {
                      "file_path": "/var/log/ecs/ecs-agent.log",
                      "log_group_name": "/${name}/ecs-agent",
                      "log_stream_name": "{instance_id}"
                    },
                    {
                      "file_path": "/var/log/ecs/audit.log",
                      "log_group_name": "/${name}/audit",
                      "log_stream_name": "{instance_id}"
                    }
                  ]
                }
              }
            }
          }
          EOL
          /opt/aws/amazon-cloudwatch-agent/bin/amazon-cloudwatch-agent-ctl -a start -m ec2
          echo "Installing SSM Agent"
          yum install -y https://s3.us-east-1.amazonaws.com/amazon-ssm-us-east-1/latest/linux_amd64/amazon-ssm-agent.rpm
`).toString('base64')),

        tagSpecifications: [{
            resourceType: "instance",
            tags: {
                ...tags,
                Name: `${name}-exec-instance`,
                Role: "exec-worker",
            },
        }],

        tags: {
            ...tags,
            Name: `${name}-exec-launch-template`,
        },
    });

    // Create launch template for SNARK agents (c7a.4xlarge)
    const snarkLaunchTemplate = new aws.ec2.LaunchTemplate(`${name}-snark-launch-template`, {
        name: `${name}-snark-launch-template`,
        imageId: standardAmi.then(ami => ami.id),
        instanceType: "c7a.4xlarge", // 16 vCPUs, 32 GB RAM, compute optimized

        iamInstanceProfile: {
            name: instanceProfile.name,
        },

        vpcSecurityGroupIds: [network.instanceSecurityGroup.id],

        blockDeviceMappings: [{
            deviceName: "/dev/xvda",
            ebs: {
                volumeSize: 100,
                volumeType: "gp3",
                encrypted: "true",
                deleteOnTermination: "true",
                iops: 3000,
                throughput: 125,
            },
        }],

        userData: cluster.name.apply(clusterName => Buffer.from(`#!/bin/bash
echo ECS_CLUSTER=${clusterName} >> /etc/ecs/ecs.config
echo ECS_ENABLE_CONTAINER_METADATA=true >> /etc/ecs/ecs.config
echo ECS_ENABLE_TASK_IAM_ROLE=true >> /etc/ecs/ecs.config
echo ECS_ENABLE_TASK_IAM_ROLE_NETWORK_HOST=true >> /etc/ecs/ecs.config
echo ECS_ENGINE_TASK_CLEANUP_WAIT_DURATION=10m >> /etc/ecs/ecs.config
echo ECS_CONTAINER_STOP_TIMEOUT=30s >> /etc/ecs/ecs.config
echo ECS_CONTAINER_START_TIMEOUT=3m >> /etc/ecs/ecs.config
echo ECS_DISABLE_PRIVILEGED=false >> /etc/ecs/ecs.config
`).toString('base64')),

        tagSpecifications: [{
            resourceType: "instance",
            tags: {
                ...tags,
                Name: `${name}-snark-instance`,
                Role: "snark-worker",
            },
        }],

        tags: {
            ...tags,
            Name: `${name}-snark-launch-template`,
        },
    });

    // Create launch template for GPU instances (g6e.xlarge)
    const gpuLaunchTemplate = new aws.ec2.LaunchTemplate(`${name}-gpu-launch-template`, {
        name: `${name}-gpu-launch-template`,
        imageId: "ami-016d360a89daa11ba",
        instanceType: "g6e.xlarge", // 4 vCPUs, 16 GB RAM, 1x NVIDIA L40S GPU

        iamInstanceProfile: {
            name: instanceProfile.name,
        },

        vpcSecurityGroupIds: [network.instanceSecurityGroup.id],

        blockDeviceMappings: [{
            deviceName: "/dev/sda1",
            ebs: {
                volumeSize: 300, // More space for GPU drivers
                volumeType: "gp3",
                encrypted: "true",
                deleteOnTermination: "true",
                iops: 3000,
                throughput: 125,
            },
        }],

        userData: cluster.name.apply(clusterName => Buffer.from(`#!/bin/bash
set -euxo pipefail
export SUDO_USER=ubuntu
export HOME=/home/ubuntu

# Update packages and install prerequisites)
apt-get update -y
apt-get install -y curl git acl awscli jq
snap install --edge --classic just

# Create directory for broker data
mkdir -p /opt/boundless/data
chown -R ubuntu:ubuntu /opt/boundless
setfacl -R -d -m u::rwx,g::rwx,o::rx /opt/boundless
setfacl -R -m u::rwx,g::rwx,o::rx /opt/boundless
chown -R ubuntu:ubuntu /opt/boundless
chmod 775 /opt/boundless

# Clone the Boundless repository
cd /opt/boundless
git clone https://github.com/boundless-xyz/boundless.git repo
cd repo
git checkout main
git config --global --add safe.directory /opt/boundless/repo

# Run Boundless setup scripts to install dependencies (docker, nvidia-container-toolkit, etc).
chmod +x /opt/boundless/repo/scripts/setup.sh
/opt/boundless/repo/scripts/setup.sh

# Download the latest ECS agent deb package for amd64 (adjust for arm64 if needed)
curl -O https://s3.us-west-2.amazonaws.com/amazon-ecs-agent-us-west-2/amazon-ecs-init-latest.amd64.deb

# Install the deb package
dpkg -i amazon-ecs-init-latest.amd64.deb

# Configure ECS agent settings
echo ECS_CLUSTER=${clusterName} >> /etc/ecs/ecs.config
echo ECS_ENABLE_GPU_SUPPORT=true >> /etc/ecs/ecs.config
echo ECS_ENABLE_CONTAINER_METADATA=true >> /etc/ecs/ecs.config
echo ECS_ENABLE_TASK_IAM_ROLE=true >> /etc/ecs/ecs.config
echo ECS_ENABLE_TASK_IAM_ROLE_NETWORK_HOST=true >> /etc/ecs/ecs.config
echo ECS_ENGINE_TASK_CLEANUP_WAIT_DURATION=10m >> /etc/ecs/ecs.config
echo ECS_CONTAINER_STOP_TIMEOUT=30s >> /etc/ecs/ecs.config
echo ECS_CONTAINER_START_TIMEOUT=3m >> /etc/ecs/ecs.config
echo ECS_DISABLE_PRIVILEGED=false >> /etc/ecs/ecs.config

mkdir -p /etc/systemd/system/ecs.service.d
cat <<EOF | sudo tee /etc/systemd/system/ecs.service.d/override.conf
[Unit]
After=docker.service
Requires=docker.service
EOF

# Reload systemd and start/enable the ECS service
systemctl daemon-reload
systemctl enable --now --no-block ecs.service
# systemctl restart ecs --no-block

# Clean up the deb file
rm amazon-ecs-init-latest.amd64.deb

reboot
`).toString('base64')),

        tagSpecifications: [{
            resourceType: "instance",
            tags: {
                ...tags,
                Name: `${name}-gpu-instance`,
                Role: "gpu-worker",
            },
        }],

        tags: {
            ...tags,
            Name: `${name}-gpu-launch-template`,
        },
    });

    // Create Auto Scaling Group for exec agents (1 instance)
    const execAsg = new aws.autoscaling.Group(`${name}-exec-asg`, {
        name: `${name}-exec-asg`,
        desiredCapacity: 1,
        maxSize: 1,
        minSize: 1,

        vpcZoneIdentifiers: network.privateSubnetIds,

        launchTemplate: {
            id: execLaunchTemplate.id,
            version: pulumi.interpolate`${execLaunchTemplate.latestVersion}`,
        },

        instanceRefresh: {
            strategy: "Rolling",
            preferences: {
                minHealthyPercentage: 0,
            },
        },

        healthCheckType: "EC2",
        healthCheckGracePeriod: 300,

        enabledMetrics: [
            "GroupInServiceInstances",
            "GroupTotalInstances",
        ],

        tags: [{
            key: "Name",
            value: `${name}-exec-asg`,
            propagateAtLaunch: false,
        }, {
            key: "AmazonECSManaged",
            value: "true",
            propagateAtLaunch: false,
        }],
    });

    // Create Auto Scaling Group for SNARK agents (1 instance)
    const snarkAsg = new aws.autoscaling.Group(`${name}-snark-asg`, {
        name: `${name}-snark-asg`,
        desiredCapacity: 1,
        maxSize: 1,
        minSize: 1,

        vpcZoneIdentifiers: network.privateSubnetIds,

        launchTemplate: {
            id: snarkLaunchTemplate.id,
            version: pulumi.interpolate`${snarkLaunchTemplate.latestVersion}`,
        },

        instanceRefresh: {
            strategy: "Rolling",
            preferences: {
                minHealthyPercentage: 0,
            },
        },

        healthCheckType: "EC2",
        healthCheckGracePeriod: 300,

        enabledMetrics: [
            "GroupInServiceInstances",
            "GroupTotalInstances",
        ],

        tags: [{
            key: "Name",
            value: `${name}-snark-asg`,
            propagateAtLaunch: false,
        }, {
            key: "AmazonECSManaged",
            value: "true",
            propagateAtLaunch: false,
        }],
    });

    // Create Auto Scaling Group for GPU instances (8 instances)
    const gpuAsg = new aws.autoscaling.Group(`${name}-gpu-asg`, {
        name: `${name}-gpu-asg`,
        desiredCapacity: 8,
        maxSize: 8,
        minSize: 8,

        vpcZoneIdentifiers: network.privateSubnetIds,

        launchTemplate: {
            id: gpuLaunchTemplate.id,
            version: pulumi.interpolate`${gpuLaunchTemplate.latestVersion}`,
        },

        instanceRefresh: {
            strategy: "Rolling",
            preferences: {
                minHealthyPercentage: 0,
            },
        },

        healthCheckType: "EC2",
        healthCheckGracePeriod: 300,

        enabledMetrics: [
            "GroupInServiceInstances",
            "GroupTotalInstances",
        ],

        tags: [{
            key: "Name",
            value: `${name}-gpu-asg`,
            propagateAtLaunch: false,
        }, {
            key: "AmazonECSManaged",
            value: "true",
            propagateAtLaunch: false,
        }],
    }, {
        replaceOnChanges: ["vpcZoneIdentifiers"]
    });

    // Create capacity providers
    const execCapacityProvider = new aws.ecs.CapacityProvider(`${name}-exec-capacity-provider`, {
        name: `${name}-exec-capacity-provider`,

        autoScalingGroupProvider: {
            autoScalingGroupArn: execAsg.arn,
            managedScaling: {
                status: "DISABLED", // No autoscaling
            },
            managedTerminationProtection: "DISABLED",
        },

        tags: {
            ...tags,
            Name: `${name}-exec-capacity-provider`,
        },
    }, {
        dependsOn: [execAsg]
    });

    const snarkCapacityProvider = new aws.ecs.CapacityProvider(`${name}-snark-capacity-provider`, {
        name: `${name}-snark-capacity-provider`,

        autoScalingGroupProvider: {
            autoScalingGroupArn: snarkAsg.arn,
            managedScaling: {
                status: "DISABLED", // No autoscaling
            },
            managedTerminationProtection: "DISABLED",
        },

        tags: {
            ...tags,
            Name: `${name}-snark-capacity-provider`,
        },
    }, {
        dependsOn: [snarkAsg]
    });

    const gpuCapacityProvider = new aws.ecs.CapacityProvider(`${name}-gpu-capacity-provider`, {
        name: `${name}-gpu-capacity-provider`,

        autoScalingGroupProvider: {
            autoScalingGroupArn: gpuAsg.arn,
            managedScaling: {
                status: "DISABLED", // No autoscaling
            },
            managedTerminationProtection: "DISABLED",
        },

        tags: {
            ...tags,
            Name: `${name}-gpu-capacity-provider`,
        },
    }, {
        dependsOn: [gpuAsg]
    });

    // Attach capacity providers to cluster
    const clusterCapacityProviders = new aws.ecs.ClusterCapacityProviders(`${name}-cluster-capacity-providers`, {
        clusterName: cluster.name,
        capacityProviders: [
            execCapacityProvider.name,
            snarkCapacityProvider.name,
            gpuCapacityProvider.name,
            "FARGATE"
        ],

        defaultCapacityProviderStrategies: [{
            capacityProvider: "FARGATE",
            weight: 1,
        }],
    });

    return {
        cluster,
        execCapacityProvider,
        snarkCapacityProvider,
        gpuCapacityProvider,
        execAsg,
        snarkAsg,
        gpuAsg,
        instanceProfile,
        gpuCompatibleSubnets: network.privateSubnetIds,
    };
}