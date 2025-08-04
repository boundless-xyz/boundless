import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { createEcsTaskRole } from "./iam";

// TODO: Enable typing
export async function setupAuxAgent(
    name: string,
    network: any,
    cluster: any,
    database: any,
    cache: any,
    storage: any,
    secrets: any,
    tags: Record<string, string>
) {
    // Get region info
    const region = await aws.getRegion().then(r => r.name);

    // Create IAM roles
    const { taskRole, executionRole } = await createEcsTaskRole(`${name}-aux-agent`, tags, secrets);

    // Create CloudWatch log group
    const logGroup = new aws.cloudwatch.LogGroup(`${name}-aux-agent-logs`, {
        name: `/aws/ecs/${name}-aux-agent`,
        retentionInDays: 7,
        tags: {
            ...tags,
            Name: `/aws/ecs/${name}-aux-agent`,
        },
    });

    // Create task definition for aux agent
    const taskDefinition = new aws.ecs.TaskDefinition(`${name}-aux-agent-task`, {
        family: `${name}-aux-agent`,
        networkMode: "awsvpc",
        requiresCompatibilities: ["FARGATE"],
        cpu: "1024", // 1 vCPU
        memory: "2048", // 2 GB
        taskRoleArn: taskRole.arn,
        executionRoleArn: executionRole.arn,

        containerDefinitions: pulumi.all([
            database.connectionUrl,
            cache.connectionUrl,
            storage.bucketName,
            storage.s3AccessKeyId,
            storage.s3SecretKey,
            logGroup.name,
            secrets.dockerToken,
        ]).apply(([dbUrl, redisUrl, s3Bucket, s3AccessKeyId, s3SecretKey, logGroupName, dockerTokenArn]) => JSON.stringify([{
            name: "aux-agent",
            // TODO: Have this in Pulumi config
            image: "risczero/risc0-bento-agent:2.3.0",
            repositoryCredentials: {
                credentialsParameter: dockerTokenArn,
            },
            command: ["-t", "aux"],
            essential: true,
            // TODO: Secrets should be provided via secrets parameter
            // e.g. https://github.com/boundless-xyz/boundless/blob/main/infra/prover/components/bonsaiBroker.ts#L391
            environment: [
                { name: "DATABASE_URL", value: dbUrl },
                { name: "REDIS_URL", value: redisUrl },
                { name: "S3_URL", value: `http://s3.${region}.amazonaws.com` },
                { name: "S3_BUCKET", value: s3Bucket },
                { name: "AWS_DEFAULT_REGION", value: region },
                { name: "RUST_LOG", value: "info" },
                { name: "RUST_BACKTRACE", value: "1" },
                { name: "LD_LIBRARY_PATH", value: "/usr/local/cuda-12.2/compat/:$LD_LIBRARY_PATH" },
                { name: "S3_ACCESS_KEY", value: s3AccessKeyId },
                { name: "S3_SECRET_KEY", value: s3SecretKey },
            ],

            logConfiguration: {
                logDriver: "awslogs",
                options: {
                    "awslogs-group": logGroupName,
                    "awslogs-region": region,
                    "awslogs-stream-prefix": "aux-agent",
                },
            },

            healthCheck: {
                command: ["CMD-SHELL", "pgrep -f '/app/agent' || exit 1"],
                interval: 30,
                timeout: 10,
                retries: 5,
                startPeriod: 120, // Allow more time for startup
            },
        }])),

        tags: {
            ...tags,
            Name: `${name}-aux-agent-task`,
        },
    });

    // Create ECS service
    const service = new aws.ecs.Service(`${name}-aux-agent-service`, {
        name: `${name}-aux-agent`,
        cluster: cluster.cluster.id,
        taskDefinition: taskDefinition.arn,
        desiredCount: 1,
        launchType: "FARGATE",

        networkConfiguration: {
            subnets: network.privateSubnetIds,
            securityGroups: [network.instanceSecurityGroup.id],
            assignPublicIp: false,
        },

        waitForSteadyState: false,

        enableEcsManagedTags: true,
        propagateTags: "SERVICE",

        tags: {
            ...tags,
            Name: `${name}-aux-agent-service`,
        },
    });

    return {
        task: taskDefinition,
        service,
        logGroup,
    };
}