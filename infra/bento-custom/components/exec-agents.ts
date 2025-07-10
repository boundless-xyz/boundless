import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { createEcsTaskRole } from "./iam";

export async function setupExecAgents(
    name: string,
    network: any,
    cluster: any,
    database: any,
    cache: any,
    storage: any,
    secrets: any,
    tags: Record<string, string>
) {
    const region = await aws.getRegion().then(r => r.name);

    // Create IAM roles for ECS
    const { taskRole, executionRole } = await createEcsTaskRole(`${name}-exec-agents`, tags);

    // Create CloudWatch log group
    const logGroup = new aws.cloudwatch.LogGroup(`${name}-exec-agents-logs`, {
        name: `/aws/ecs/${name}-exec-agents`,
        retentionInDays: 7,
        tags: {
            ...tags,
            Name: `/aws/ecs/${name}-exec-agents`,
        },
    });

    // Create task definition for exec agents
    const taskDefinition = new aws.ecs.TaskDefinition(`${name}-exec-agents-task`, {
        family: `${name}-exec-agents`,
        networkMode: "awsvpc",
        requiresCompatibilities: ["EC2"],
        cpu: "6144", // 6 vCPUs out of 8 available on r7iz.2xlarge
        memory: "49152", // 48 GB out of 64 GB available
        taskRoleArn: taskRole.arn,
        executionRoleArn: executionRole.arn,

        // Placement constraints for exec instances
        placementConstraints: [{
            type: "memberOf",
            expression: "attribute:ecs.instance-type == r7iz.2xlarge",
        }],

        containerDefinitions: pulumi.all([
            database.connectionUrl,
            cache.connectionUrl,
            storage.bucketName,
            storage.s3AccessKeyId,
            storage.s3SecretKey,
            logGroup.name,
        ]).apply(([dbUrl, redisUrl, s3Bucket, s3AccessKeyId, s3SecretKey, logGroupName]) => JSON.stringify([
            {
                name: "exec-agent-0",
                image: "risczero/risc0-bento-agent:stable",
                command: ["/app/agent", "-t", "exec", "--segment-po2", "21"],
                essential: true,
                memory: 12288, // 12 GB per agent
                cpu: 1536, // 1.5 vCPUs per agent

                environment: [
                    { name: "DATABASE_URL", value: dbUrl },
                    { name: "REDIS_URL", value: redisUrl },
                    { name: "S3_URL", value: "https://s3.amazonaws.com" },
                    { name: "S3_BUCKET", value: s3Bucket },
                    { name: "AWS_DEFAULT_REGION", value: region },
                    { name: "RUST_LOG", value: "info" },
                    { name: "RUST_BACKTRACE", value: "1" },
                    { name: "RISC0_KECCAK_PO2", value: "17" },
                    { name: "LD_LIBRARY_PATH", value: "/usr/local/cuda-12.2/compat/" },
                    { name: "S3_ACCESS_KEY", value: s3AccessKeyId },
                    { name: "S3_SECRET_KEY", value: s3SecretKey },
                ],

                logConfiguration: {
                    logDriver: "awslogs",
                    options: {
                        "awslogs-group": logGroupName,
                        "awslogs-region": region,
                        "awslogs-stream-prefix": "exec-agent-0",
                    },
                },

                healthCheck: {
                    command: ["CMD-SHELL", "pgrep -f '/app/agent' || exit 1"],
                    interval: 30,
                    timeout: 10,
                    retries: 3,
                    startPeriod: 60,
                },
            },
            {
                name: "exec-agent-1",
                image: "risczero/risc0-bento-agent:stable",
                command: ["/app/agent", "-t", "exec", "--segment-po2", "21"],
                essential: true,
                memory: 12288, // 12 GB per agent
                cpu: 1536, // 1.5 vCPUs per agent

                environment: [
                    { name: "DATABASE_URL", value: dbUrl },
                    { name: "REDIS_URL", value: redisUrl },
                    { name: "S3_URL", value: "https://s3.amazonaws.com" },
                    { name: "S3_BUCKET", value: s3Bucket },
                    { name: "AWS_DEFAULT_REGION", value: region },
                    { name: "RUST_LOG", value: "info" },
                    { name: "RUST_BACKTRACE", value: "1" },
                    { name: "RISC0_KECCAK_PO2", value: "17" },
                    { name: "LD_LIBRARY_PATH", value: "/usr/local/cuda-12.2/compat/" },
                    { name: "S3_ACCESS_KEY", value: s3AccessKeyId },
                    { name: "S3_SECRET_KEY", value: s3SecretKey },
                ],

                logConfiguration: {
                    logDriver: "awslogs",
                    options: {
                        "awslogs-group": logGroupName,
                        "awslogs-region": region,
                        "awslogs-stream-prefix": "exec-agent-1",
                    },
                },

                healthCheck: {
                    command: ["CMD-SHELL", "pgrep -f '/app/agent' || exit 1"],
                    interval: 30,
                    timeout: 10,
                    retries: 3,
                    startPeriod: 60,
                },
            },
            {
                name: "exec-agent-2",
                image: "risczero/risc0-bento-agent:stable",
                command: ["/app/agent", "-t", "exec", "--segment-po2", "21"],
                essential: true,
                memory: 12288, // 12 GB per agent
                cpu: 1536, // 1.5 vCPUs per agent

                environment: [
                    { name: "DATABASE_URL", value: dbUrl },
                    { name: "REDIS_URL", value: redisUrl },
                    { name: "S3_URL", value: "https://s3.amazonaws.com" },
                    { name: "S3_BUCKET", value: s3Bucket },
                    { name: "AWS_DEFAULT_REGION", value: region },
                    { name: "RUST_LOG", value: "info" },
                    { name: "RUST_BACKTRACE", value: "1" },
                    { name: "RISC0_KECCAK_PO2", value: "17" },
                    { name: "LD_LIBRARY_PATH", value: "/usr/local/cuda-12.2/compat/" },
                    { name: "S3_ACCESS_KEY", value: s3AccessKeyId },
                    { name: "S3_SECRET_KEY", value: s3SecretKey },
                ],

                logConfiguration: {
                    logDriver: "awslogs",
                    options: {
                        "awslogs-group": logGroupName,
                        "awslogs-region": region,
                        "awslogs-stream-prefix": "exec-agent-2",
                    },
                },

                healthCheck: {
                    command: ["CMD-SHELL", "pgrep -f '/app/agent' || exit 1"],
                    interval: 30,
                    timeout: 10,
                    retries: 3,
                    startPeriod: 60,
                },
            },
            {
                name: "exec-agent-3",
                image: "risczero/risc0-bento-agent:stable",
                command: ["/app/agent", "-t", "exec", "--segment-po2", "21"],
                essential: true,
                memory: 12288, // 12 GB per agent
                cpu: 1536, // 1.5 vCPUs per agent

                environment: [
                    { name: "DATABASE_URL", value: dbUrl },
                    { name: "REDIS_URL", value: redisUrl },
                    { name: "S3_URL", value: "https://s3.amazonaws.com" },
                    { name: "S3_BUCKET", value: s3Bucket },
                    { name: "AWS_DEFAULT_REGION", value: region },
                    { name: "RUST_LOG", value: "info" },
                    { name: "RUST_BACKTRACE", value: "1" },
                    { name: "RISC0_KECCAK_PO2", value: "17" },
                    { name: "LD_LIBRARY_PATH", value: "/usr/local/cuda-12.2/compat/" },
                    { name: "S3_ACCESS_KEY", value: s3AccessKeyId },
                    { name: "S3_SECRET_KEY", value: s3SecretKey },
                ],

                logConfiguration: {
                    logDriver: "awslogs",
                    options: {
                        "awslogs-group": logGroupName,
                        "awslogs-region": region,
                        "awslogs-stream-prefix": "exec-agent-3",
                    },
                },

                healthCheck: {
                    command: ["CMD-SHELL", "pgrep -f '/app/agent' || exit 1"],
                    interval: 30,
                    timeout: 10,
                    retries: 3,
                    startPeriod: 60,
                },
            },
        ])),

        tags: {
            ...tags,
            Name: `${name}-exec-agents-task`,
        },
    });

    // Create ECS service
    const service = new aws.ecs.Service(`${name}-exec-agents-service`, {
        name: `${name}-exec-agents`,
        cluster: cluster.cluster.id,
        taskDefinition: taskDefinition.arn,
        desiredCount: 1,

        capacityProviderStrategies: [{
            capacityProvider: cluster.execCapacityProvider.name,
            weight: 1,
        }],

        networkConfiguration: {
            subnets: network.privateSubnetIds, // EC2 instances in private subnets
            securityGroups: [network.instanceSecurityGroup.id],
        },

        waitForSteadyState: false,

        enableEcsManagedTags: true,
        propagateTags: "SERVICE",

        tags: {
            ...tags,
            Name: `${name}-exec-agents-service`,
        },
    });

    return {
        taskDefinition,
        service,
        logGroup,
    };
}