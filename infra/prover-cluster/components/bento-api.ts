import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export async function setupBentoAPI(
    name: string,
    network: any,
    cluster: any,
    database: any,
    cache: any,
    storage: any,
    secrets: any,
    tags: Record<string, string>
) {
    const config = new pulumi.Config();

    // Create IAM role for Bento API task
    const taskRole = new aws.iam.Role(`${name}-bento-api-task-role`, {
        assumeRolePolicy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [{
                Action: "sts:AssumeRole",
                Effect: "Allow",
                Principal: {
                    Service: "ecs-tasks.amazonaws.com"
                }
            }]
        }),
        tags: {
            ...tags,
            Name: `${name}-bento-api-task-role`,
        },
    });

    // Create IAM role for ECS execution
    const executionRole = new aws.iam.Role(`${name}-bento-api-execution-role`, {
        assumeRolePolicy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [{
                Action: "sts:AssumeRole",
                Effect: "Allow",
                Principal: {
                    Service: "ecs-tasks.amazonaws.com"
                }
            }]
        }),
        tags: {
            ...tags,
            Name: `${name}-bento-api-execution-role`,
        },
    });

    // Attach execution role policy
    new aws.iam.RolePolicyAttachment(`${name}-bento-api-execution-policy`, {
        role: executionRole.name,
        policyArn: "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy",
    });

    // Create CloudWatch Log Group
    const logGroup = new aws.cloudwatch.LogGroup(`${name}-bento-api-logs`, {
        name: `/aws/ecs/${name}-bento-api`,
        retentionInDays: 7,
        tags: {
            ...tags,
            Name: `/aws/ecs/${name}-bento-api`,
        },
    });

    // Create ECS Task Definition
    const taskDefinition = new aws.ecs.TaskDefinition(`${name}-bento-api-task`, {
        family: `${name}-bento-api`,
        requiresCompatibilities: ["FARGATE"],
        networkMode: "awsvpc",
        cpu: "2048", // 2 vCPU
        memory: "4096", // 4GB RAM
        executionRoleArn: executionRole.arn,
        taskRoleArn: taskRole.arn,

        containerDefinitions: pulumi.all([
            database.connectionUrl,
            cache.connectionUrl,
            storage.bucket.id,
            storage.s3AccessKeyId,
            storage.s3SecretKey,
            logGroup.name
        ]).apply(([dbUrl, redisUrl, bucketId, s3AccessKeyId, s3SecretKey, logGroupName]) =>
            JSON.stringify([{
                name: "bento-api",
                image: "risczero/risc0-bento-rest-api:2.3.0",
                essential: true,

                portMappings: [{
                    containerPort: 8081,
                    protocol: "tcp"
                }],

                environment: [
                    {
                        name: "DATABASE_URL",
                        value: dbUrl
                    },
                    {
                        name: "REDIS_URL",
                        value: redisUrl
                    },
                    {
                        name: "S3_URL",
                        value: `https://s3.${config.get("aws:region") || "us-west-2"}.amazonaws.com`
                    },
                    {
                        name: "S3_BUCKET",
                        value: bucketId
                    },
                    {
                        name: "AWS_DEFAULT_REGION",
                        value: config.get("aws:region") || "us-west-2"
                    },
                    {
                        name: "RUST_LOG",
                        value: "info"
                    },
                    {
                        name: "RUST_BACKTRACE",
                        value: "1"
                    },
                    {
                        name: "SNARK_TIMEOUT",
                        value: (config.getNumber("snarkTimeout") || 180).toString()
                    },
                    {
                        name: "S3_ACCESS_KEY",
                        value: s3AccessKeyId
                    },
                    {
                        name: "S3_SECRET_KEY",
                        value: s3SecretKey
                    }
                ],

                command: [
                    "--bind-addr", "0.0.0.0:8081",
                    `--snark-timeout=${config.getNumber("snarkTimeout") || 180}`
                ],

                healthCheck: {
                    command: ["CMD-SHELL", "pgrep -f '/app/rest_api' || exit 1"],
                    interval: 30,
                    timeout: 5,
                    retries: 3,
                    startPeriod: 60
                },

                logConfiguration: {
                    logDriver: "awslogs",
                    options: {
                        "awslogs-group": logGroupName,
                        "awslogs-region": config.get("aws:region") || "us-west-2",
                        "awslogs-stream-prefix": "ecs"
                    }
                }
            }])
        ),

        tags: {
            ...tags,
            Name: `${name}-bento-api-task`,
        },
    });

    // Create Application Load Balancer Target Group first
    const targetGroup = new aws.lb.TargetGroup(`${name}-bento-api-tg`, {
        name: `${name}-bento-api-tg`,
        port: 8081,
        protocol: "HTTP",
        vpcId: network.vpc.vpcId,
        targetType: "ip",

        healthCheck: {
            enabled: true,
            path: "/healthy",
            port: "8081",
            protocol: "HTTP",
            interval: 30,
            timeout: 5,
            healthyThreshold: 2,
            unhealthyThreshold: 2,
            matcher: "200-499"
        },

        tags: {
            ...tags,
            Name: `${name}-bento-api-tg`,
        },
    });

    // Create ECS Service
    const service = new aws.ecs.Service(`${name}-bento-api-service`, {
        name: `${name}-bento-api-service`,
        cluster: cluster.cluster.id,
        taskDefinition: taskDefinition.arn,
        desiredCount: 1,
        launchType: "FARGATE",

        networkConfiguration: {
            subnets: network.vpc.privateSubnetIds,
            securityGroups: [network.instanceSecurityGroup.id],
            assignPublicIp: false,
        },

        loadBalancers: [{
            targetGroupArn: targetGroup.arn,
            containerName: "bento-api",
            containerPort: 8081,
        }],

        waitForSteadyState: false,

        tags: {
            ...tags,
            Name: `${name}-bento-api-service`,
        },
    });


    // Create Application Load Balancer (internal)
    const loadBalancer = new aws.lb.LoadBalancer(`${name}-bento-api-alb`, {
        name: `${name}-bento-api-alb`,
        loadBalancerType: "application",
        internal: true, // Internal ALB for VPC access only
        subnets: network.vpc.privateSubnetIds,
        securityGroups: [network.instanceSecurityGroup.id, network.brokerSecurityGroup.id],

        tags: {
            ...tags,
            Name: `${name}-bento-api-alb`,
        },
    });


    // Create ALB Listener
    const listener = new aws.lb.Listener(`${name}-bento-api-listener`, {
        loadBalancerArn: loadBalancer.arn,
        port: 8081,
        protocol: "HTTP",

        defaultActions: [{
            type: "forward",
            targetGroupArn: targetGroup.arn,
        }],

        tags: {
            ...tags,
            Name: `${name}-bento-api-listener`,
        },
    });

    return {
        service,
        taskDefinition,
        logGroup,
        loadBalancer,
        targetGroup,
        bentoApiUrl: pulumi.interpolate`http://${loadBalancer.dnsName}:8081`,
    };
}