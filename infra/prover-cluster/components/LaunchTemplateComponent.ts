import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as crypto from "crypto";
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
    minioUsername: string;
    minioPassword: string;
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
            config.minioUsername,
            config.minioPassword,
            config.ethRpcUrl!,
            config.privateKey!,
            config.orderStreamUrl!,
            config.verifierAddress!,
            config.boundlessMarketAddress!,
            config.setVerifierAddress!,
            config.collateralTokenAddress!,
            config.chainId!,
            this.config.stackName,
            config.componentType
        ]).apply(([dbName, dbUser, dbPass, minioUser, minioPass, rpcUrl, privKey, orderStreamUrl, verifierAddress, boundlessMarketAddress, setVerifierAddress, collateralTokenAddress, chainId, stackName, componentType]) => {
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
echo "S3_BUCKET=bento" >> /etc/environment
echo "S3_URL=http://localhost:9000" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "S3_ACCESS_KEY=${minioUser}" >> /etc/environment
echo "S3_SECRET_KEY=${minioPass}" >> /etc/environment
echo "STACK_NAME=${stackName}" >> /etc/environment
echo "COMPONENT_TYPE=${componentType}" >> /etc/environment
/usr/bin/sed -i 's|group_name: "/boundless/bent.*"|group_name: "/boundless/bento/${stackName}/${componentType}"|g' /etc/vector/vector.yaml

# Ethereum configuration
echo "RPC_URL=${rpcUrl}" >> /etc/environment
echo "PRIVATE_KEY=${privKey}" >> /etc/environment
# Public order stream URL
echo "ORDER_STREAM_URL=${orderStreamUrl}" >> /etc/environment
# Base contract addresses
echo "VERIFIER_ADDRESS=${verifierAddress}" >> /etc/environment
echo "BOUNDLESS_MARKET_ADDRESS=${boundlessMarketAddress}" >> /etc/environment
echo "SET_VERIFIER_ADDRESS=${setVerifierAddress}" >> /etc/environment
echo "COLLATERAL_TOKEN_ADDRESS=${collateralTokenAddress}" >> /etc/environment
echo "CHAIN_ID=${chainId}" >> /etc/environment

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
docker run -d \\
  --name boundless-postgres \\
  --restart unless-stopped \\
  -e POSTGRES_DB=${dbName} \\
  -e POSTGRES_USER=${dbUser} \\
  -e POSTGRES_PASSWORD=${dbPass} \\
  -p 5432:5432 \\
  -v /opt/boundless/data/postgres:/var/lib/postgresql/data \\
  postgres:16

# Start Redis container
docker run -d \\
  --name boundless-redis \\
  --restart unless-stopped \\
  -p 6379:6379 \\
  -v /opt/boundless/data/redis:/data \\
  redis:7-alpine redis-server --appendonly yes

# Start MinIO container
docker run -d \\
  --name boundless-minio \\
  --restart unless-stopped \\
  -p 9000:9000 \\
  -p 9001:9001 \\
  -e MINIO_ROOT_USER=${minioUser} \\
  -e MINIO_ROOT_PASSWORD=${minioPass} \\
  -v /opt/boundless/data/minio:/data \\
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
systemctl restart vector
systemctl start bento-api.service bento-broker.service
systemctl enable bento-api.service bento-broker.service`;
            return Buffer.from(userDataScript).toString('base64');
        });
    }

    private generateWorkerUserData(config: LaunchTemplateConfig): pulumi.Output<string> {
        return pulumi.all([
            config.managerIp!,
            config.taskDBName,
            config.taskDBUsername,
            config.taskDBPassword,
            config.minioUsername,
            config.minioPassword,
            this.config.stackName,
            config.componentType
        ]).apply(([managerIp, dbName, dbUser, dbPass, minioUser, minioPass, stackName, componentType]) => {
            const commonEnvVars = this.generateCommonEnvVars(managerIp, dbName, dbUser, dbPass, minioUser, minioPass, stackName, componentType);

            let componentSpecificVars = "";
            let serviceFile = "";

            switch (config.componentType) {
                case "prover":
                    serviceFile = "bento-prover.service";
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
systemctl start bento.service
systemctl enable bento.service`;

            return Buffer.from(userDataScript).toString('base64');
        });
    }

    private generateCommonEnvVars(
        managerIp: string,
        dbName: string,
        dbUser: string,
        dbPass: string,
        minioUser: string,
        minioPass: string,
        stackName: string,
        componentType: string,
    ): string {
        return `# Database and Redis URLs
echo "DATABASE_URL=postgresql://${dbUser}:${dbPass}@${managerIp}:5432/${dbName}" >> /etc/environment
echo "REDIS_URL=redis://${managerIp}:6379" >> /etc/environment

# S3 Configuration - using MinIO on manager
echo "RUST_LOG=info" >> /etc/environment
echo "S3_BUCKET=bento" >> /etc/environment
echo "S3_URL=http://${managerIp}:9000" >> /etc/environment
echo "AWS_REGION=us-west-2" >> /etc/environment
echo "S3_ACCESS_KEY=${minioUser}" >> /etc/environment
echo "S3_SECRET_KEY=${minioPass}" >> /etc/environment
echo "REDIS_TTL=57600" >> /etc/environment
echo "STACK_NAME=${stackName}" >> /etc/environment
echo "COMPONENT_TYPE=${componentType}" >> /etc/environment
/usr/bin/sed -i 's|group_name: "/boundless/bent.*"|group_name: "/boundless/bento/${stackName}/${componentType}"|g' /etc/vector/vector.yaml`;
    }
}
