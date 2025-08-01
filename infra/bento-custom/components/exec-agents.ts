import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { createEcsTaskRole } from "./iam";
import { Secrets } from "./secrets";
import { Network } from "./network";
import { Storage } from "./storage";


function execAgentContainerDef(
    dbUrl: string,
    redisUrl: string,
    s3Bucket: string,
    s3AccessKeyId: string,
    s3SecretKey: string,
    logGroupName: string,
    dockerTokenArn: string,
    region: string,
    n: number
) {
    return [...Array(n)].map((_, i) => {
        return {
            name: `exec-agent-${i}`,
            image: "risczero/risc0-bento-agent:2.3.0",
            repositoryCredentials: {
                credentialsParameter: dockerTokenArn,
            },
            command: ["-t", "exec", "--segment-po2", "21"],
            essential: true,
            memory: 12288, // 12 GB per agent
            cpu: 1536, // 1.5 vCPUs per agent

            environment: [
                { name: "DATABASE_URL", value: dbUrl },
                { name: "REDIS_URL", value: redisUrl },
                { name: "S3_URL", value: `http://s3.${region}.amazonaws.com` },
                { name: "S3_BUCKET", value: s3Bucket },
                { name: "AWS_DEFAULT_REGION", value: region },
                { name: "RUST_LOG", value: "info" },
                { name: "RISC0_INFO", value: "1" },
                { name: "RUST_BACKTRACE", value: "1" },
                { name: "RISC0_KECCAK_PO2", value: "17" },
                { name: "LD_LIBRARY_PATH", value: "/usr/local/cuda-12.2/compat/:$LD_LIBRARY_PATH" },
                { name: "S3_ACCESS_KEY", value: s3AccessKeyId },
                { name: "S3_SECRET_KEY", value: s3SecretKey },
            ],

            restartPolicy: {
                enabled: true,
                restartAttemptPeriod: 60, // Restart after 60 seconds if the agent fails
            },

            logConfiguration: {
                logDriver: "awslogs",
                options: {
                    "awslogs-group": logGroupName,
                    "awslogs-region": region,
                    "awslogs-stream-prefix": `exec-agent-${i}`,
                },
            },

            healthCheck: {
                command: ["CMD-SHELL", "pgrep -f '/app/agent' || exit 1"],
                interval: 30,
                timeout: 10,
                retries: 3,
                startPeriod: 60,
            },
        };
    });
}

export async function setupExecAgents(
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
    const { taskRole, executionRole } = await createEcsTaskRole(`${name}-exec-agents`, tags, secrets);

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
            secrets.dockerToken,
        ]).apply(([dbUrl, redisUrl, s3Bucket, s3AccessKeyId, s3SecretKey, logGroupName, dockerTokenArn]) => JSON.stringify(
            execAgentContainerDef(dbUrl, redisUrl, s3Bucket, s3AccessKeyId, s3SecretKey, logGroupName, dockerTokenArn, region, 4)
        )),

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