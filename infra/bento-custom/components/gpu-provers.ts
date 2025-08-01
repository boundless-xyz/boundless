import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { createEcsTaskRole } from "./iam";
import { Network } from "./network";
import { Storage } from "./storage";
import { Secrets } from "./secrets";

export async function setupGpuProvers(
    name: string,
    network: Network,
    cluster: any,
    database: any,
    cache: any,
    storage: Storage,
    secrets: Secrets,
    tags: Record<string, string>
) {
    const region = await aws.getRegion().then(r => r.name);

    // Create IAM roles for ECS
    const { taskRole, executionRole } = await createEcsTaskRole(`${name}-gpu-provers`, tags, secrets);

    // Create CloudWatch log group
    const logGroup = new aws.cloudwatch.LogGroup(`${name}-gpu-provers-logs`, {
        name: `/aws/ecs/${name}-gpu-provers`,
        retentionInDays: 7,
        tags: {
            ...tags,
            Name: `/aws/ecs/${name}-gpu-provers`,
        },
    });

    // Create task definition for GPU provers
    const taskDefinition = new aws.ecs.TaskDefinition(`${name}-gpu-provers-task`, {
        family: `${name}-gpu-provers`,
        networkMode: "awsvpc",
        requiresCompatibilities: ["EC2"],
        taskRoleArn: taskRole.arn,
        executionRoleArn: executionRole.arn,

        // Placement constraints for GPU instances
        placementConstraints: [{
            type: "memberOf",
            expression: "attribute:ecs.instance-type == g6e.xlarge",
        }],

        containerDefinitions: pulumi.all([
            database.connectionUrl,
            cache.connectionUrl,
            storage.bucketName,
            storage.s3AccessKeyId,
            storage.s3SecretKey,
            logGroup.name,
            secrets.dockerToken,
        ]).apply(([dbUrl, redisUrl, s3Bucket, s3AccessKeyId, s3SecretKey, logGroupName, dockerTokenArn]) => JSON.stringify([
            {
                name: "gpu-prover",
                image: "risczero/risc0-bento-agent:2.3.0",
                repositoryCredentials: {
                    credentialsParameter: dockerTokenArn,
                },
                command: ["-t", "prove"],
                essential: true,
                cpu: 3584, // 3.5 vCPUs out of 4 available on g6e.xlarge
                memory: 14336, // 14 GB out of 16 GB available

                resourceRequirements: [
                    {
                        type: "GPU",
                        value: "1",
                    },
                ],

                environment: [
                    { name: "DATABASE_URL", value: dbUrl },
                    { name: "REDIS_URL", value: redisUrl },
                    { name: "S3_URL", value: `http://s3.${region}.amazonaws.com` },
                    { name: "S3_BUCKET", value: s3Bucket },
                    { name: "AWS_DEFAULT_REGION", value: region },
                    { name: "RUST_LOG", value: "info" },
                    { name: "RISC0_INFO", value: "1" },
                    { name: "RUST_BACKTRACE", value: "1" },
                    { name: "LD_LIBRARY_PATH", value: "/usr/local/cuda/lib64:$LD_LIBRARY_PATH" },
                    { name: "S3_ACCESS_KEY", value: s3AccessKeyId },
                    { name: "S3_SECRET_KEY", value: s3SecretKey },
                ],

                logConfiguration: {
                    logDriver: "awslogs",
                    options: {
                        "awslogs-group": logGroupName,
                        "awslogs-region": region,
                        "awslogs-stream-prefix": "gpu-prover",
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
            Name: `${name}-gpu-provers-task`,
        },
    });

    // Create ECS service for GPU provers
    const service = new aws.ecs.Service(`${name}-gpu-provers-service`, {
        name: `${name}-gpu-provers`,
        cluster: cluster.cluster.id,
        taskDefinition: taskDefinition.arn,
        desiredCount: 8, // 8 GPU instances as specified in README

        capacityProviderStrategies: [{
            capacityProvider: cluster.gpuCapacityProvider.name,
            weight: 1,
        }],

        networkConfiguration: {
            subnets: network.privateSubnetIds, // GPU instances in private subnets
            securityGroups: [network.instanceSecurityGroup.id],
        },

        waitForSteadyState: false,

        enableEcsManagedTags: true,
        propagateTags: "SERVICE",

        tags: {
            ...tags,
            Name: `${name}-gpu-provers-service`,
        },
    });

    return {
        taskDefinition,
        service,
        logGroup,
    };
}