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

// Create a new EC2 instance
const server = new aws.ec2.Instance("builder", {
    instanceType: "c5a.2xlarge",
    keyName: sshKey.keyName,
    ami: "ami-087f352c165340ea1", // Amazon Linux 2 AMI
    vpcSecurityGroupIds: [securityGroup.id],
    tags: {
        Name: "builder",
    },
    rootBlockDevice: {
      volumeSize: 200,
      volumeType: "gp3",
    },
    userDataReplaceOnChange: false,
    userData: 
    `#!/bin/bash
set -e -v

# Update and install dependenciess
yum update -y
yum install -y docker git

# Start and enable Docker
systemctl start docker
systemctl enable docker

# Add ec2-user to the docker group
usermod -aG docker ec2-user

# Increase the size of the /tmp partition, used by docker
mount -o remount,size=12G /tmp

# Reduce size of /dev/shm, not important for a docker builder
mount -o remount,size=4G /dev/shm
    `,
});

export const stateBucket = bucket.id;
export const secretKey = keyAlias.arn;
export const publicIp = server.publicIp;
export const publicHostName = server.publicDns;