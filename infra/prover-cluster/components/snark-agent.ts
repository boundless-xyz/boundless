import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { createEcsTaskRole } from "./iam";

export async function setupSnarkAgent(
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
    const { taskRole, executionRole } = await createEcsTaskRole(`${name}-snark-agent`, tags, secrets);

    // Create CloudWatch log group
    const logGroup = new aws.cloudwatch.LogGroup(`${name}-snark-agent-logs`, {
        name: `/aws/ecs/${name}-snark-agent`,
        retentionInDays: 7,
        tags: {
            ...tags,
            Name: `/aws/ecs/${name}-snark-agent`,
        },
    });

    // Create task definition for SNARK agent
    const taskDefinition = new aws.ecs.TaskDefinition(`${name}-snark-agent-task`, {
        family: `${name}-snark-agent`,
        networkMode: "awsvpc",
        requiresCompatibilities: ["EC2"],
        taskRoleArn: taskRole.arn,
        executionRoleArn: executionRole.arn,

        // Placement constraints for SNARK instances
        placementConstraints: [{
            type: "memberOf",
            expression: "attribute:ecs.instance-type == c7a.4xlarge",
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
                name: "snark-agent",
                image: "risczero/risc0-bento-agent:2.3.0",
                repositoryCredentials: {
                    credentialsParameter: dockerTokenArn,
                },
                command: ["-t", "snark"],
                essential: true,
                cpu: 15360, // 15 vCPUs out of 16 available on c7a.4xlarge
                memory: 28672, // 28 GB out of 32 GB available

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

                ulimits: [
                    {
                        name: "stack",
                        softLimit: 90000000,
                        hardLimit: 90000000,
                    },
                ],

                logConfiguration: {
                    logDriver: "awslogs",
                    options: {
                        "awslogs-group": logGroupName,
                        "awslogs-region": region,
                        "awslogs-stream-prefix": "snark-agent",
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
            Name: `${name}-snark-agent-task`,
        },
    });

    // Create ECS service
    const service = new aws.ecs.Service(`${name}-snark-agent-service`, {
        name: `${name}-snark-agent`,
        cluster: cluster.cluster.id,
        taskDefinition: taskDefinition.arn,
        desiredCount: 1,

        capacityProviderStrategies: [{
            capacityProvider: cluster.snarkCapacityProvider.name,
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
            Name: `${name}-snark-agent-service`,
        },
    });

    return {
        taskDefinition,
        service,
        logGroup,
    };
}