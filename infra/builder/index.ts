import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { createPulumiState } from "./pulumiResources";

const config = new pulumi.Config();
const publicKey = config.requireSecret('PUBLIC_KEY');

const { bucket, keyAlias } = createPulumiState();

// Generate an SSH key pair
const sshKey = new aws.ec2.KeyPair("ssh-key", {
    publicKey: publicKey,
});

// Create a new security group for our server
const securityGroup = new aws.ec2.SecurityGroup("builder-sec", {
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
});

// Create a new EC2 instance with instance store
const serverLocal = new aws.ec2.Instance("builder-local", {
    instanceType: "c6id.2xlarge", // Using c6id.2xlarge which has 16GB RAM and 237GB NVMe SSD
    keyName: sshKey.keyName,
    ami: "ami-087f352c165340ea1", // Amazon Linux 2 AMI
    vpcSecurityGroupIds: [securityGroup.id],
    tags: {
        Name: "builder-local",
    },
    userDataReplaceOnChange: true,
    userData:
        `#!/bin/bash
set -e -v

# Update and install dependencies
yum update -y
yum install -y docker git

# Start and enable Docker
systemctl start docker
systemctl enable docker

# Add ec2-user to the docker group
usermod -aG docker ec2-user

# Format and mount the instance store volume
mkfs -t xfs /dev/nvme1n1

# Create mount point
mkdir -p /mnt/docker-local

# Mount the volume
mount /dev/nvme1n1 /mnt/docker-local

# Add mount information to fstab using UUID for stability across reboots
UUID=$(blkid -p -s UUID -o value "/dev/nvme1n1")
if ! grep -q "$UUID" /etc/fstab; then
    echo "UUID=$UUID /mnt/docker-local xfs defaults,nofail 0 2" >> /etc/fstab
fi

# Configure Docker to use the instance store
mkdir -p /mnt/docker-local/docker

# Update Docker daemon configuration to use the instance store
cat > /etc/docker/daemon.json << EOF
{
  "data-root": "/mnt/docker-local/docker",
  "storage-driver": "overlay2"
}
EOF

# Restart Docker to apply changes
systemctl restart docker

    `,
});

export const stateBucket = bucket.id;
export const secretKey = keyAlias.arn;
export const publicIpLocal = serverLocal.publicIp;
export const publicHostNameLocal = serverLocal.publicDns;