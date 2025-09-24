import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as fs from "fs";

const stackName = pulumi.getStack();
const baseConfig = new pulumi.Config("base");
// Pulumi shared outputs from the bootstrap stack
const baseStackName = baseConfig.require('BASE_STACK');
const baseStack = new pulumi.StackReference(baseStackName);
const vpcId = baseStack.getOutput('VPC_ID');
const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');

const config = new pulumi.Config();
const environment = config.get("environment") || "custom";

// Required configuration
const privateKey = config.requireSecret("privateKey");
const ethRpcUrl = config.requireSecret("ethRpcUrl");
const orderStreamUrl = config.require("orderStreamUrl");
const imageId = config.require("imageId");
const managerInstanceType = config.require("managerInstanceType");
// Removed spot market configuration - using g6.xlarge for all provers

// Contract addresses
const taskDBUsername = config.require("taskDBUsername");
const taskDBPassword = config.require("taskDBPassword");
const taskDBName = config.require("taskDBName");

// MinIO configuration
const minioUsername = config.get("minioUsername") || "minioadmin";
const minioPassword = config.get("minioPassword") || "minioadmin123";

const executionCount = config.getNumber("executionCount") || 1;
const proverCount = config.getNumber("proverWorkerCount") || 1;
const auxCount = config.getNumber("auxWorkerCount") || 1;

// 1) Instance role & profile (SSM access)
const ec2Role = new aws.iam.Role("ec2SsmRole", {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({ Service: "ec2.amazonaws.com" }),
});

// Attach SSM managed instance core policy
new aws.iam.RolePolicyAttachment("attachSsmCore", {
    role: ec2Role.name,
    policyArn: "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
});

// Attach additional policies for CloudWatch and S3 access
new aws.iam.RolePolicyAttachment("ec2-cloudwatch-policy", {
    role: ec2Role.name,
    policyArn: "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
});

new aws.iam.RolePolicyAttachment("ec2-s3-policy", {
    role: ec2Role.name,
    policyArn: "arn:aws:iam::aws:policy/AmazonS3FullAccess",
});

// Create instance profile
const ec2Profile = new aws.iam.InstanceProfile("ec2Profile", {
    role: ec2Role.name
});

// Create S3 bucket for boundless data
const s3Bucket = new aws.s3.Bucket("boundless-bento-bucket", {
    bucket: `boundless-bento-${environment}-${pulumi.getStack()}`,
    tags: {
        Name: "boundless-bento-data",
        Environment: environment,
        Project: "boundless-bento-cluster",
    },
});

// Create S3 bucket versioning
const s3BucketVersioning = new aws.s3.BucketVersioningV2("boundless-bento-bucket-versioning", {
    bucket: s3Bucket.id,
    versioningConfiguration: {
        status: "Enabled",
    },
});

// Create S3 bucket server-side encryption
const s3BucketServerSideEncryption = new aws.s3.BucketServerSideEncryptionConfigurationV2("boundless-bento-bucket-encryption", {
    bucket: s3Bucket.id,
    rules: [{
        applyServerSideEncryptionByDefault: {
            sseAlgorithm: "AES256",
        },
    }],
});

const securityGroup = new aws.ec2.SecurityGroup("manager-sg", {
    vpcId: vpcId,
    description: "Security group for manager",
    ingress: [
        {
            protocol: "tcp",
            fromPort: 22,
            toPort: 22,
            cidrBlocks: ["0.0.0.0/0"],
            description: "SSH access"
        },
        {
            protocol: "tcp",
            fromPort: 6379,
            toPort: 6379,
            self: true,
            description: "Redis access from same security group"
        },
        {
            protocol: "tcp",
            fromPort: 5432,
            toPort: 5432,
            self: true,
            description: "PostgreSQL access from same security group"
        },
        {
            protocol: "tcp",
            fromPort: 8080,
            toPort: 8080,
            cidrBlocks: ["0.0.0.0/0"],
            description: "Bento API access"
        },
        {
            protocol: "tcp",
            fromPort: 9000,
            toPort: 9000,
            self: true,
            description: "MinIO S3 API access from same security group"
        },
        {
            protocol: "tcp",
            fromPort: 9001,
            toPort: 9001,
            cidrBlocks: ["0.0.0.0/0"],
            description: "MinIO Console access"
        }
    ],
    egress: [
        {
            protocol: "-1",
            fromPort: 0,
            toPort: 0,
            cidrBlocks: ["0.0.0.0/0"],
            description: "All outbound traffic"
        }
    ],
});

const manager = new aws.ec2.Instance("manager", {
    ami: imageId,
    instanceType: managerInstanceType,
    subnetId: privSubNetIds.apply((subnets: any) => subnets[0]),
    vpcSecurityGroupIds: [securityGroup.id],
    iamInstanceProfile: ec2Profile.name,
    userData: pulumi.all([taskDBName, taskDBUsername, taskDBPassword, s3Bucket.bucket, minioUsername, minioPassword, ethRpcUrl, privateKey]).apply(([dbName, dbUser, dbPass, bucketName, minioUser, minioPass, rpcUrl, privKey]) => {
        const userDataScript = `#!/bin/bash
# Set environment variables
echo "RUST_LOG=info" >> /etc/environment
echo "BENTO_API_LISTEN_ADDR=0.0.0.0" >> /etc/environment
echo "BENTO_API_PORT=8081" >> /etc/environment
echo "SNARK_TIMEOUT=1800" >> /etc/environment
echo "BENTO_BROKER_LISTEN_ADDR=0.0.0.0" >> /etc/environment
echo "BENTO_BROKER_PORT=8082" >> /etc/environment
echo "BENTO_EXECUTOR_LISTEN_ADDR=0.0.0.0" >> /etc/environment
echo "BENTO_EXECUTOR_PORT=8083" >> /etc/environment
echo "BENTO_PROVER_LISTEN_ADDR=0.0.0.0" >> /etc/environment
echo "BENTO_PROVER_PORT=8086" >> /etc/environment

# Database and Redis URLs for manager (localhost)
echo "DATABASE_URL=postgresql://${dbUser}:${dbPass}@localhost:5432/${dbName}" >> /etc/environment
echo "REDIS_URL=redis://localhost:6379" >> /etc/environment

# S3 Configuration - using MinIO
echo "S3_BUCKET=${bucketName}" >> /etc/environment
echo "S3_URL=http://localhost:9000" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "S3_ACCESS_KEY=${minioUser}" >> /etc/environment
echo "S3_SECRET_KEY=${minioPass}" >> /etc/environment

# Ethereum configuration
echo "RPC_URL=${rpcUrl}" >> /etc/environment
echo "PRIVATE_KEY=${privKey}" >> /etc/environment

# Copy and configure service files
cp /opt/boundless/config/bento-api.service /etc/systemd/system/bento-api.service
cp /opt/boundless/config/bento.service /etc/systemd/system/bento.service
cp /opt/boundless/config/bento-broker.service /etc/systemd/system/bento-broker.service

# Install Docker
apt-get update
apt-get install -y docker.io
systemctl start docker
systemctl enable docker

# Create data directories for Docker volumes
mkdir -p /opt/boundless/data/postgres
mkdir -p /opt/boundless/data/redis
mkdir -p /opt/boundless/data/minio

# Set proper ownership of /opt/boundless
chown -R ubuntu:ubuntu /opt/boundless

# Start PostgreSQL container
docker run -d \
  --name boundless-postgres \
  --restart unless-stopped \
  -e POSTGRES_DB=${dbName} \
  -e POSTGRES_USER=${dbUser} \
  -e POSTGRES_PASSWORD=${dbPass} \
  -p 5432:5432 \
  -v /opt/boundless/data/postgres:/var/lib/postgresql/data \
  postgres:16

# Start Redis container
docker run -d \
  --name boundless-redis \
  --restart unless-stopped \
  -p 6379:6379 \
  -v /opt/boundless/data/redis:/data \
  redis:7-alpine redis-server --appendonly yes

# Start MinIO container
docker run -d \
  --name boundless-minio \
  --restart unless-stopped \
  -p 9000:9000 \
  -p 9001:9001 \
  -e MINIO_ROOT_USER=${minioUser} \
  -e MINIO_ROOT_PASSWORD=${minioPass} \
  -v /opt/boundless/data/minio:/data \
  minio/minio server /data --console-address ":9001"

# Wait for containers to be ready
sleep 10

# Health check
echo "Performing health checks..."
# Check PostgreSQL
docker exec boundless-postgres psql -U ${dbUser} -d ${dbName} -c "SELECT 1;" > /dev/null 2>&1 && echo "PostgreSQL is running" || echo "PostgreSQL health check failed"
# Check Redis
docker exec boundless-redis redis-cli ping > /dev/null 2>&1 && echo "Redis is running" || echo "Redis health check failed"
# Check MinIO
curl -f http://localhost:9000/minio/health/live > /dev/null 2>&1 && echo "MinIO is running" || echo "MinIO health check failed"

systemctl daemon-reload
systemctl start bento-api.service bento-broker.service
systemctl enable bento-api.service bento-broker.service`;
        return Buffer.from(userDataScript).toString('base64');
    }),
    userDataReplaceOnChange: false,
    ebsBlockDevices: [{
        deviceName: "/dev/sda1",
        volumeSize: 1024,
        volumeType: "gp3",
        deleteOnTermination: true,
    }],
    tags: {
        Name: "boundless-bento-manager",
        Type: "manager",
        Environment: environment,
        Project: "boundless-bento-cluster",
        "ssm:bootstrap": "manager",
    },
});

const proverLaunchTemplate = new aws.ec2.LaunchTemplate("prover-launch-template", {
    name: `boundless-bento-prover-template-${environment}`,
    imageId: imageId,
    instanceType: "g6.xlarge",
    vpcSecurityGroupIds: [securityGroup.id],
    iamInstanceProfile: {
        name: ec2Profile.name,
    },
    userData: pulumi.all([manager.privateIp, taskDBName, taskDBUsername, taskDBPassword, s3Bucket.bucket, minioUsername, minioPassword, ethRpcUrl, privateKey]).apply(([managerIp, dbName, dbUser, dbPass, bucketName, minioUser, minioPass, rpcUrl, privKey]) => {
        const userDataScript = `#!/bin/bash
# Database and Redis URLs for prover (point to manager)
echo "DATABASE_URL=postgresql://${dbUser}:${dbPass}@${managerIp}:5432/${dbName}" >> /etc/environment
echo "REDIS_URL=redis://${managerIp}:6379" >> /etc/environment

# S3 Configuration - using MinIO on manager
echo "RUST_LOG=info" >> /etc/environment
echo "S3_BUCKET=${bucketName}" >> /etc/environment
echo "S3_URL=http://${managerIp}:9000" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "S3_ACCESS_KEY=${minioUser}" >> /etc/environment
echo "S3_SECRET_KEY=${minioPass}" >> /etc/environment

# Copy and configure service file
cp /opt/boundless/config/bento-prover.service /etc/systemd/system/bento.service
systemctl daemon-reload
systemctl start bento.service
systemctl enable bento.service`;
        return Buffer.from(userDataScript).toString('base64');
    }),
    blockDeviceMappings: [{
        deviceName: "/dev/sda1",
        ebs: {
            volumeSize: 100,
            volumeType: "gp3",
            deleteOnTermination: "true",
        },
    }],
    tagSpecifications: [{
        resourceType: "instance",
        tags: {
            Name: "boundless-bento-prover",
            Type: "prover",
            Environment: environment,
            Project: "boundless-bento-cluster",
            InstanceType: "g6.xlarge",
            "ssm:bootstrap": "prover",
        },
    }],
});

// Auto Scaling Group
const proverAsg = new aws.autoscaling.Group("prover-asg", {
    name: `boundless-bento-prover-asg-${environment}`,
    vpcZoneIdentifiers: privSubNetIds,
    minSize: proverCount,
    maxSize: proverCount,
    desiredCapacity: proverCount,
    launchTemplate: {
        id: proverLaunchTemplate.id,
        version: "$Latest",
    },
    healthCheckType: "EC2",
    healthCheckGracePeriod: 300,
    defaultCooldown: 300,
    terminationPolicies: ["OldestInstance"],
    tags: [
        {
            key: "Name",
            value: "boundless-bento-prover-asg",
            propagateAtLaunch: false,
        },
        {
            key: "Type",
            value: "prover-asg",
            propagateAtLaunch: false,
        },
        {
            key: "Environment",
            value: environment,
            propagateAtLaunch: true,
        },
        {
            key: "Project",
            value: "boundless-bento-cluster",
            propagateAtLaunch: true,
        },
    ],
});

const executionLaunchTemplate = new aws.ec2.LaunchTemplate("execution-launch-template", {
    name: `boundless-bento-execution-template-${environment}`,
    imageId: imageId,
    instanceType: "c7i.large",
    vpcSecurityGroupIds: [securityGroup.id],
    iamInstanceProfile: {
        name: ec2Profile.name,
    },
    userData: pulumi.all([manager.privateIp, taskDBName, taskDBUsername, taskDBPassword, s3Bucket.bucket, minioUsername, minioPassword, ethRpcUrl, privateKey]).apply(([managerIp, dbName, dbUser, dbPass, bucketName, minioUser, minioPass, rpcUrl, privKey]) => {
        const userDataScript = `#!/bin/bash
# Database and Redis URLs for execution (point to manager)
echo "DATABASE_URL=postgresql://${dbUser}:${dbPass}@${managerIp}:5432/${dbName}" >> /etc/environment
echo "REDIS_URL=redis://${managerIp}:6379" >> /etc/environment

# S3 Configuration - using MinIO on manager
echo "RUST_LOG=info" >> /etc/environment
echo "S3_BUCKET=${bucketName}" >> /etc/environment
echo "S3_URL=http://${managerIp}:9000" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "S3_ACCESS_KEY=${minioUser}" >> /etc/environment
echo "S3_SECRET_KEY=${minioPass}" >> /etc/environment
echo "FINALIZE_RETRIES=3" >> /etc/environment
echo "FINALIZE_TIMEOUT=60" >> /etc/environment

# Copy and configure service file
cp /opt/boundless/config/bento-executor.service /etc/systemd/system/bento.service
systemctl daemon-reload
systemctl start bento.service
systemctl enable bento.service`;
        return Buffer.from(userDataScript).toString('base64');
    }),
    blockDeviceMappings: [{
        deviceName: "/dev/sda1",
        ebs: {
            volumeSize: 100,
            volumeType: "gp3",
            deleteOnTermination: "true",
        },
    }],
    tagSpecifications: [{
        resourceType: "instance",
        tags: {
            Name: "boundless-bento-execution",
            Type: "execution",
            Environment: environment,
            Project: "boundless-bento-cluster",
            InstanceType: "c7i.large",
            "ssm:bootstrap": "execution",
        },
    }],
});

// Auto Scaling Group
const executionAsg = new aws.autoscaling.Group("execution-asg", {
    name: `boundless-bento-execution-asg-${environment}`,
    vpcZoneIdentifiers: privSubNetIds,
    minSize: executionCount,
    maxSize: executionCount,
    desiredCapacity: executionCount,
    launchTemplate: {
        id: executionLaunchTemplate.id,
        version: "$Latest",
    },
    healthCheckType: "EC2",
    healthCheckGracePeriod: 300,
    defaultCooldown: 300,
    terminationPolicies: ["OldestInstance"],
    tags: [
        {
            key: "Name",
            value: "boundless-bento-execution-asg",
            propagateAtLaunch: false,
        },
        {
            key: "Type",
            value: "execution-asg",
            propagateAtLaunch: false,
        },
        {
            key: "Environment",
            value: environment,
            propagateAtLaunch: true,
        },
        {
            key: "Project",
            value: "boundless-bento-cluster",
            propagateAtLaunch: true,
        },
    ],
});

// Launch template for aux agent instances
const auxLaunchTemplate = new aws.ec2.LaunchTemplate("aux-launch-template", {
    name: `boundless-bento-aux-template-${environment}`,
    imageId: imageId,
    instanceType: "t3.medium", // Smaller instance for aux agent
    vpcSecurityGroupIds: [securityGroup.id],
    iamInstanceProfile: {
        name: ec2Profile.name,
    },
    userData: pulumi.all([manager.privateIp, taskDBName, taskDBUsername, taskDBPassword, s3Bucket.bucket, minioUsername, minioPassword, ethRpcUrl, privateKey]).apply(([managerIp, dbName, dbUser, dbPass, bucketName, minioUser, minioPass, rpcUrl, privKey]) => {
        const userDataScript = `#!/bin/bash
# Database and Redis URLs for aux agent (point to manager)
echo "DATABASE_URL=postgresql://${dbUser}:${dbPass}@${managerIp}:5432/${dbName}" >> /etc/environment
echo "REDIS_URL=redis://${managerIp}:6379" >> /etc/environment

# S3 Configuration - using MinIO on manager
echo "RUST_LOG=info" >> /etc/environment
echo "S3_BUCKET=${bucketName}" >> /etc/environment
echo "S3_URL=http://${managerIp}:9000" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "S3_ACCESS_KEY=${minioUser}" >> /etc/environment
echo "S3_SECRET_KEY=${minioPass}" >> /etc/environment

# Copy and configure service file
cp /opt/boundless/config/bento-aux.service /etc/systemd/system/bento.service
systemctl daemon-reload
systemctl start bento.service
systemctl enable bento.service`;
        return Buffer.from(userDataScript).toString('base64');
    }),
    blockDeviceMappings: [{
        deviceName: "/dev/sda1",
        ebs: {
            volumeSize: 100,
            volumeType: "gp3",
            deleteOnTermination: "true",
        },
    }],
    tagSpecifications: [{
        resourceType: "instance",
        tags: {
            Name: "boundless-bento-aux-agent",
            Environment: environment,
            Component: "aux-agent",
        },
    }],
});

// Auto Scaling Group for aux agent
const auxAsg = new aws.autoscaling.Group("aux-asg", {
    name: `boundless-bento-aux-asg-${environment}`,
    vpcZoneIdentifiers: privSubNetIds,
    minSize: auxCount,
    maxSize: auxCount,
    desiredCapacity: auxCount,
    launchTemplate: {
        id: auxLaunchTemplate.id,
        version: "$Latest",
    },
    healthCheckType: "EC2",
    healthCheckGracePeriod: 300,
    defaultCooldown: 300,
    terminationPolicies: ["OldestInstance"],
    tags: [
        {
            key: "Name",
            value: "boundless-bento-aux-asg",
            propagateAtLaunch: true,
        },
        {
            key: "Environment",
            value: environment,
            propagateAtLaunch: true,
        },
        {
            key: "Component",
            value: "aux-agent",
            propagateAtLaunch: true,
        },
    ],
});


// Outputs
export const managerInstanceId = manager.id;
export const managerPrivateIp = manager.privateIp;
export const managerPublicIp = manager.publicIp;

// ASG outputs
export const proverAsgName = proverAsg.name;
export const proverAsgArn = proverAsg.arn;
export const proverDesiredCapacity = proverAsg.desiredCapacity;
export const proverMinSize = proverAsg.minSize;
export const proverMaxSize = proverAsg.maxSize;

// Execution ASG outputs
export const executionAsgName = executionAsg.name;
export const executionAsgArn = executionAsg.arn;
export const executionDesiredCapacity = executionAsg.desiredCapacity;
export const executionMinSize = executionAsg.minSize;
export const executionMaxSize = executionAsg.maxSize;

// Aux ASG outputs
export const auxAsgName = auxAsg.name;
export const auxAsgArn = auxAsg.arn;
export const auxDesiredCapacity = auxAsg.desiredCapacity;
export const auxMinSize = auxAsg.minSize;
export const auxMaxSize = auxAsg.maxSize;

// Redis connection details
export const redisHost = manager.privateIp;
export const redisPort = "6379";

// S3 bucket details
export const s3BucketName = s3Bucket.bucket;
export const s3BucketArn = s3Bucket.arn;

// Shared credentials for prover nodes
export const sharedCredentials = {
    postgresHost: manager.privateIp,
    postgresPort: "5432",
    postgresDb: taskDBName,
    postgresUser: taskDBUsername,
    postgresPassword: taskDBPassword,
    redisHost: manager.privateIp,
    redisPort: "6379",
    s3Bucket: s3Bucket.bucket,
    s3Region: "us-west-2",
};

// Cluster info
export const clusterInfo = {
    manager: {
        instanceId: manager.id,
        publicIp: manager.publicIp,
        privateIp: manager.privateIp,
    },
    proverAsg: {
        name: proverAsg.name,
        arn: proverAsg.arn,
        desiredCapacity: proverAsg.desiredCapacity,
        minSize: proverAsg.minSize,
        maxSize: proverAsg.maxSize,
        instanceType: "g6.xlarge",
    },
    s3: {
        bucketName: s3Bucket.bucket,
        bucketArn: s3Bucket.arn,
        region: "us-west-2",
    },
};
