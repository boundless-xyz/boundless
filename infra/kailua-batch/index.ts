import * as aws from "@pulumi/aws";
import * as awsx from "@pulumi/awsx";
import * as pulumi from "@pulumi/pulumi";
import { getServiceNameV1, Severity } from "../util";

require("dotenv").config();

type SecretRef = {
    name: string;
    valueFrom: pulumi.Input<string>;
};

type SecretMapping = {
    configKey: string;
    envName: string;
    fallbackConfigKey?: string;
};

// Pulumi config keys -> container env var names (see Pulumi.*.yaml).
const REQUIRED_SECRETS: SecretMapping[] = [
    { configKey: "WALLET_KEY", envName: "BOUNDLESS_WALLET_KEY" },
    { configKey: "L1_RPC", envName: "L1_RPC" },
    { configKey: "L2_RPC", envName: "L2_RPC" },
    { configKey: "OP_NODE", envName: "OP_NODE" },
    { configKey: "L1_BEACON", envName: "L1_BEACON" },
    { configKey: "S3_URL", envName: "S3_URL" },
    // Accept either config key name; container env is always S3_ACCESS_KEY / S3_SECRET_KEY.
    { configKey: "S3_ACCESS_KEY", envName: "S3_ACCESS_KEY", fallbackConfigKey: "S3_UPLOAD_ACCESS_KEY" },
    { configKey: "S3_SECRET_KEY", envName: "S3_SECRET_KEY", fallbackConfigKey: "S3_SECRET_ACCESS_KEY" },
    { configKey: "BOUNDLESS_RPC_URL", envName: "BOUNDLESS_RPC_URL" },
];

const OPTIONAL_SECRETS: SecretMapping[] = [];

/** Staggered schedules: one per minute 0–19 in each 20-minute window. */
const SCHEDULE_COUNT = 20;

type StaggeredScheduleSpec = {
    /** EventBridge cron minute (0–19); fires at :MM, :MM+20, :MM+40 each hour. */
    minute: number;
    /** Passed to the task as START_BLOCK_OFFSET → start_block = finalized - minute. */
    startBlockOffset: number;
    blockCount: string;
    cronExpression: string;
};

function buildStaggeredSchedules(): StaggeredScheduleSpec[] {
    return Array.from({ length: SCHEDULE_COUNT }, (_, minute) => ({
        minute,
        startBlockOffset: minute,
        blockCount: minute % 2 === 0 ? "1" : "2",
        cronExpression: `cron(${minute}/20 * * * ? *)`,
    }));
}

const STAGGERED_SCHEDULES = buildStaggeredSchedules();

export = () => {
    const config = new pulumi.Config();
    const awsConfig = new pulumi.Config("aws");
    const region = awsConfig.require("region");
    const stackName = pulumi.getStack();

    const chainId = config.require("CHAIN_ID");
    const serviceName = getServiceNameV1(stackName, "kailua-batch", chainId);
    const baseStackName = config.require("BASE_STACK");
    const baseStack = new pulumi.StackReference(baseStackName);
    const vpcId = baseStack.getOutput("VPC_ID") as pulumi.Output<string>;
    const privateSubnetIds = baseStack.getOutput("PRIVATE_SUBNET_IDS") as pulumi.Output<string[]>;

    const image = config.require("KAILUA_IMAGE");
    const defaultBlockCount = config.get("BLOCK_COUNT") ?? "2";
    const taskCpu = (parseInt(config.get("TASK_CPU") ?? config.get("JOB_VCPU") ?? "4", 10) * 1024);
    const taskMemory = parseInt(config.get("TASK_MEMORY_MIB") ?? config.get("JOB_MEMORY_MIB") ?? "8192", 10);
    const logRetentionDays = config.getNumber("LOG_RETENTION_DAYS") ?? 30;
    const kailuaWorkdir = config.get("KAILUA_WORKDIR") ?? "/kailua";
    const assignPublicIp = config.getBoolean("ASSIGN_PUBLIC_IP") ?? false;
    const maxRunningTasks = config.getNumber("MAX_RUNNING_TASKS")
        ?? parseInt(config.get("NUM_CONCURRENT_PROVERS") ?? "3", 10);

    const slackTopicArn = config.get("SLACK_ALERTS_TOPIC_ARN");
    const pagerdutyTopicArn = config.get("PAGERDUTY_ALERTS_TOPIC_ARN");
    const enablePagerDuty = config.getBoolean("ENABLE_PAGERDUTY_ALERTS") ?? false;
    const alarmActions = [slackTopicArn, enablePagerDuty ? pagerdutyTopicArn : undefined].filter(Boolean) as string[];

    const logGroup = new aws.cloudwatch.LogGroup(`${serviceName}-logs`, {
        name: serviceName,
        retentionInDays: logRetentionDays,
    });

    const securityGroup = new aws.ec2.SecurityGroup(`${serviceName}-security-group`, {
        name: serviceName,
        vpcId,
        egress: [
            {
                fromPort: 0,
                toPort: 0,
                protocol: "-1",
                cidrBlocks: ["0.0.0.0/0"],
                ipv6CidrBlocks: ["::/0"],
            },
        ],
    });

    const executionRole = new aws.iam.Role(`${serviceName}-execution-role`, {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
            Service: "ecs-tasks.amazonaws.com",
        }),
        managedPolicyArns: [aws.iam.ManagedPolicy.AmazonECSTaskExecutionRolePolicy],
    });

    const taskRole = new aws.iam.Role(`${serviceName}-task-role`, {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
            Service: "ecs-tasks.amazonaws.com",
        }),
    });

    const taskSecrets = [
        ...REQUIRED_SECRETS.map(({ configKey, envName, fallbackConfigKey }) =>
            requiredSecret(configKey, envName, fallbackConfigKey)),
        ...OPTIONAL_SECRETS.map(({ configKey, envName, fallbackConfigKey }) =>
            optionalSecret(configKey, envName, fallbackConfigKey)),
    ].filter((secret): secret is SecretRef => secret !== undefined);

    const accountId = aws.getCallerIdentityOutput().accountId;
    const logGroupArn = pulumi.interpolate`arn:aws:logs:${region}:${accountId}:log-group:${logGroup.name}:*`;

    new aws.iam.RolePolicy(`${serviceName}-execution-logs-policy`, {
        role: executionRole.id,
        policy: {
            Version: "2012-10-17",
            Statement: [
                {
                    Effect: "Allow",
                    Action: [
                        "logs:CreateLogStream",
                        "logs:PutLogEvents",
                    ],
                    Resource: logGroupArn,
                },
            ],
        },
    });

    if (taskSecrets.length > 0) {
        new aws.iam.RolePolicy(`${serviceName}-execution-secrets-policy`, {
            role: executionRole.id,
            policy: pulumi
                .all(taskSecrets.map(secret => secret.valueFrom))
                .apply(secretArns => JSON.stringify({
                    Version: "2012-10-17",
                    Statement: [
                        {
                            Effect: "Allow",
                            Action: ["secretsmanager:GetSecretValue"],
                            Resource: secretArns.map(arn => secretsManagerIamResource(arn)),
                        },
                    ],
                })),
        });
    }

    const orderStreamUrl = config.get("BOUNDLESS_ORDER_STREAM_URL");
    if (!orderStreamUrl) {
        throw new Error("BOUNDLESS_ORDER_STREAM_URL must be set in stack config");
    }

    const defaultEnvironment: Record<string, string> = {
        MODE: config.get("MODE") ?? "debug",
        BOUNDLESS_ORDER_STREAM_URL: orderStreamUrl,
        BOUNDLESS_ORDER_SUBMISSION_COOLDOWN: "1",
        EXPORT_PROFILE_CSV: "true",
        BOUNDLESS_ORDER_FUNDING_MODE: config.get("BOUNDLESS_ORDER_FUNDING_MODE") ?? "available-balance",
        ENABLE_EXPERIMENTAL_WITNESS_ENDPOINT: "true",
        BOUNDLESS_LOOK_BACK: config.get("BOUNDLESS_LOOK_BACK") ?? "false",
        MAX_WITNESS_SIZE: "47185920",
        NO_COLOR: "1",
        R2_DOMAIN: config.require("R2_DOMAIN"),
        BOUNDLESS_DYNAMIC_PRICING: config.get("BOUNDLESS_DYNAMIC_PRICING") ?? "true",
        S3_BUCKET: config.require("S3_BUCKET"),
        AWS_REGION: config.get("AWS_REGION") ?? "auto",
        LOG_VERBOSITY: config.get("LOG_VERBOSITY") ?? "-vvv",
        STORAGE_PROVIDER: "s3",
        BLOCK_COUNT: defaultBlockCount,
        S3_USE_PRESIGNED: config.get("S3_USE_PRESIGNED") ?? "true",
        START_BLOCK_OFFSET: "0",
        RISC0_INFO: "1",
        SKIP_DERIVATION_PROOF: config.get("SKIP_DERIVATION_PROOF") ?? "true",
        // Submit Boundless orders without polling until Fulfilled (avoids Locked retry loop).
        SKIP_AWAIT_PROOF: config.get("SKIP_AWAIT_PROOF") ?? "true",
        DATA_REL: config.get("DATA_REL") ?? ".localtestdata/op-mainnet",
        NUM_TAIL_BLOCKS: config.get("NUM_TAIL_BLOCKS") ?? "32",
        NUM_CONCURRENT_PROOFS: config.get("NUM_CONCURRENT_PROOFS") ?? "1",
        NUM_CONCURRENT_PROVERS: config.get("NUM_CONCURRENT_PROVERS") ?? "3",
        SEQ_WINDOW: config.get("SEQ_WINDOW") ?? "50",
    };

    const extraEnvironment = config.getObject<Record<string, string>>("ENV") ?? {};
    const environment = Object.entries({ ...defaultEnvironment, ...extraEnvironment })
        .map(([name, value]) => ({ name, value }));

    const ecsSecrets = taskSecrets.map(secret => ({
        name: secret.name,
        valueFrom: secret.valueFrom,
    }));

    // Matches the image's `just prove` workflow (cast finalized block + env-driven args).
    // Override entirely with kailua-batch:KAILUA_COMMAND in stack config if needed.
    const defaultProveScript = [
        ': "${S3_ACCESS_KEY:?S3_ACCESS_KEY is required for R2}"',
        ': "${S3_SECRET_KEY:?S3_SECRET_KEY is required for R2}"',
        'export AWS_ACCESS_KEY_ID="${S3_ACCESS_KEY}"',
        'export AWS_SECRET_ACCESS_KEY="${S3_SECRET_KEY}"',
        'FINALIZED=$(cast bn finalized -r "$L2_RPC")',
        'START_BLOCK=$((FINALIZED - START_BLOCK_OFFSET))',
        'echo "KAILUA_RUNNING finalized=${FINALIZED} start_block=${START_BLOCK} offset=${START_BLOCK_OFFSET} block_count=${BLOCK_COUNT}"',
        `cd ${kailuaWorkdir}`,
        'just prove "$START_BLOCK" "$BLOCK_COUNT" "$L1_RPC" "$L1_BEACON" "$L2_RPC" "$OP_NODE" "$DATA_REL" "$MODE" "$SEQ_WINDOW" "$LOG_VERBOSITY" 1 || [ $? -eq 111 ]',
    ].join("\n");

    const containerCommand = config.get("KAILUA_COMMAND") ?? [
        "set -eu",
        defaultProveScript,
    ].join("\n");

    const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, { name: serviceName });

    const fargateTask = new awsx.ecs.FargateTaskDefinition(
        `${serviceName}-task`,
        {
            family: serviceName,
            container: {
                name: serviceName,
                image,
                cpu: taskCpu,
                memory: taskMemory,
                essential: true,
                entryPoint: ["/bin/sh", "-c"],
                command: [containerCommand],
                environment,
                secrets: ecsSecrets,
            },
            logGroup: {
                existing: logGroup,
            },
            executionRole: {
                roleArn: executionRole.arn,
            },
            taskRole: {
                roleArn: taskRole.arn,
            },
        },
        { dependsOn: [executionRole, logGroup] },
    );

    const triggerLambdaRole = new aws.iam.Role(`${serviceName}-trigger-lambda-role`, {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
            Service: "lambda.amazonaws.com",
        }),
        managedPolicyArns: [aws.iam.ManagedPolicy.AWSLambdaBasicExecutionRole],
    });

    new aws.iam.RolePolicy(`${serviceName}-trigger-lambda-policy`, {
        role: triggerLambdaRole.id,
        policy: pulumi
            .all([cluster.arn, fargateTask.taskDefinition.arn, executionRole.arn, taskRole.arn])
            .apply(([clusterArn, taskDefinitionArn, executionRoleArn, taskRoleArn]) => JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: ["ecs:ListTasks"],
                        Resource: "*",
                        Condition: {
                            ArnEquals: {
                                "ecs:cluster": clusterArn,
                            },
                        },
                    },
                    {
                        Effect: "Allow",
                        Action: ["ecs:RunTask"],
                        Resource: [
                            taskDefinitionArn.replace(/:\d+$/, ":*"),
                            clusterArn,
                        ],
                    },
                    {
                        Effect: "Allow",
                        Action: ["iam:PassRole"],
                        Resource: [executionRoleArn, taskRoleArn],
                    },
                ],
            })),
    });

    const assignPublicIpValue = assignPublicIp ? "ENABLED" : "DISABLED";

    // Custom name (not /aws/lambda/...) — Lambda auto-creates the default path on first run,
    // which conflicts with Pulumi-managed LogGroups at that path.
    const triggerLambdaLogGroup = new aws.cloudwatch.LogGroup(`${serviceName}-trigger-logs`, {
        name: pulumi.interpolate`${serviceName}-trigger`,
        retentionInDays: logRetentionDays,
    });

    const triggerLambda = new aws.lambda.Function(`${serviceName}-trigger`, {
        name: `${serviceName}-trigger`,
        role: triggerLambdaRole.arn,
        runtime: "nodejs20.x",
        handler: "index.handler",
        timeout: 30,
        loggingConfig: {
            logGroup: triggerLambdaLogGroup.name,
            logFormat: "Text",
        },
        code: new pulumi.asset.AssetArchive({
            ".": new pulumi.asset.FileArchive("trigger-lambda/build"),
        }),
        environment: {
            variables: {
                CLUSTER_ARN: cluster.arn,
                TASK_DEFINITION_FAMILY: serviceName,
                TASK_DEFINITION_ARN: fargateTask.taskDefinition.arn,
                CONTAINER_NAME: serviceName,
                SUBNET_IDS: privateSubnetIds.apply(ids => ids.join(",")),
                SECURITY_GROUP_ID: securityGroup.id,
                ASSIGN_PUBLIC_IP: assignPublicIpValue,
                MAX_RUNNING_TASKS: maxRunningTasks.toString(),
            },
        },
    }, { dependsOn: [triggerLambdaRole, fargateTask, triggerLambdaLogGroup] });

    const schedulerRole = new aws.iam.Role(`${serviceName}-scheduler-role`, {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
            Service: "scheduler.amazonaws.com",
        }),
    });

    new aws.iam.RolePolicy(`${serviceName}-scheduler-policy`, {
        role: schedulerRole.id,
        policy: triggerLambda.arn.apply(lambdaArn => JSON.stringify({
            Version: "2012-10-17",
            Statement: [
                {
                    Effect: "Allow",
                    Action: ["lambda:InvokeFunction"],
                    Resource: lambdaArn,
                },
            ],
        })),
    });

    for (const spec of STAGGERED_SCHEDULES) {
        const { minute, startBlockOffset, blockCount, cronExpression } = spec;
        const schedulePayload = JSON.stringify({
            startBlockOffset,
            blockCount,
        });
        new aws.scheduler.Schedule(`${serviceName}-minute-${minute}`, {
            name: `${serviceName}-minute-${minute}`,
            description: `Minute ${minute}: START_BLOCK_OFFSET=${startBlockOffset}, BLOCK_COUNT=${blockCount}`,
            flexibleTimeWindow: {
                mode: "OFF",
            },
            scheduleExpression: cronExpression,
            target: {
                arn: "arn:aws:scheduler:::aws-sdk:lambda:invoke",
                roleArn: schedulerRole.arn,
                input: triggerLambda.arn.apply(lambdaArn => JSON.stringify({
                    FunctionName: lambdaArn,
                    InvocationType: "Event",
                    Payload: schedulePayload,
                })),
            },
        }, { dependsOn: [triggerLambda] });
    }

    const metricsNamespace = `Boundless/Services/${serviceName}`;
    createMetricFilter("running", "\"KAILUA_RUNNING\"", `${serviceName}-running`);
    createMetricFilter("error", "ERROR", `${serviceName}-log-err`);
    createMetricFilter("bal-eth", "\"[B-BAL-ETH]\"", `${serviceName}-log-bal-eth`);
    createMetricFilter("bal-stk", "\"[B-BAL-STK]\"", `${serviceName}-log-bal-stk`);

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-task-skipped-filter`, {
        name: `${serviceName}-task-skipped-filter`,
        logGroupName: triggerLambdaLogGroup.name,
        metricTransformation: {
            namespace: metricsNamespace,
            name: `${serviceName}-task-skipped`,
            value: "1",
            defaultValue: "0",
        },
        pattern: "KAILUA_TASK_SKIPPED",
    }, { dependsOn: [triggerLambda, triggerLambdaLogGroup] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-not-running-alarm-${Severity.SEV2}`, {
        name: `${serviceName}-not-running-${Severity.SEV2}`,
        namespace: metricsNamespace,
        metricName: `${serviceName}-running`,
        statistic: "Sum",
        period: 3600,
        threshold: 1,
        comparisonOperator: "LessThanThreshold",
        evaluationPeriods: 1,
        datapointsToAlarm: 1,
        treatMissingData: "breaching",
        alarmDescription: `${Severity.SEV2}: Kailua did not start any Fargate task in 1 hour`,
        actionsEnabled: true,
        alarmActions,
    });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-error-alarm-${Severity.SEV2}`, {
        name: `${serviceName}-log-err-${Severity.SEV2}`,
        namespace: metricsNamespace,
        metricName: `${serviceName}-log-err`,
        statistic: "Sum",
        period: 1200,
        threshold: 1,
        comparisonOperator: "GreaterThanOrEqualToThreshold",
        evaluationPeriods: 3,
        datapointsToAlarm: 3,
        treatMissingData: "notBreaching",
        alarmDescription: `${Severity.SEV2}: Kailua logged ERROR in 3 consecutive 20-minute periods`,
        actionsEnabled: true,
        alarmActions,
    });

    for (const balanceMetricName of [`${serviceName}-log-bal-eth`, `${serviceName}-log-bal-stk`]) {
        new aws.cloudwatch.MetricAlarm(`${balanceMetricName}-alarm-${Severity.SEV2}`, {
            name: `${balanceMetricName}-${Severity.SEV2}`,
            namespace: metricsNamespace,
            metricName: balanceMetricName,
            statistic: "Sum",
            period: 3600,
            threshold: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            treatMissingData: "notBreaching",
            alarmDescription: `${Severity.SEV2}: Kailua emitted a low-funds log in 1 hour`,
            actionsEnabled: true,
            alarmActions,
        });
    }

    return {
        clusterArn: cluster.arn,
        taskDefinitionArn: fargateTask.taskDefinition.arn,
        logGroupName: logGroup.name,
        logsConsoleUrl: logGroup.name.apply(
            name => `https://${region}.console.aws.amazon.com/cloudwatch/home?region=${region}#logsV2:log-groups/log-group/${encodeURIComponent(name)}`,
        ),
        schedules: STAGGERED_SCHEDULES,
    };

    function resolveConfigKey(configKey: string, fallbackConfigKey?: string): string {
        if (config.get(`${configKey}_SECRET_ARN`) || config.getSecret(configKey)) {
            return configKey;
        }
        if (fallbackConfigKey && (config.get(`${fallbackConfigKey}_SECRET_ARN`) || config.getSecret(fallbackConfigKey))) {
            return fallbackConfigKey;
        }
        return configKey;
    }

    function requiredSecret(configKey: string, envName: string, fallbackConfigKey?: string): SecretRef {
        const resolvedKey = resolveConfigKey(configKey, fallbackConfigKey);
        const arn = config.get(`${resolvedKey}_SECRET_ARN`);
        if (arn) {
            return { name: envName, valueFrom: arn };
        }

        const value = config.requireSecret(resolvedKey);
        const secretId = resolvedKey.toLowerCase().replace(/_/g, "-");
        const secret = new aws.secretsmanager.Secret(`${serviceName}-${secretId}`);
        new aws.secretsmanager.SecretVersion(`${serviceName}-${secretId}-v1`, {
            secretId: secret.id,
            secretString: value,
        });
        return { name: envName, valueFrom: secret.arn };
    }

    function optionalSecret(configKey: string, envName: string, fallbackConfigKey?: string): SecretRef | undefined {
        const resolvedKey = resolveConfigKey(configKey, fallbackConfigKey);
        const arn = config.get(`${resolvedKey}_SECRET_ARN`);
        if (arn) {
            return { name: envName, valueFrom: arn };
        }

        const value = config.getSecret(resolvedKey);
        if (!value) {
            return undefined;
        }

        const secretId = resolvedKey.toLowerCase().replace(/_/g, "-");
        const secret = new aws.secretsmanager.Secret(`${serviceName}-${secretId}`);
        new aws.secretsmanager.SecretVersion(`${serviceName}-${secretId}-v1`, {
            secretId: secret.id,
            secretString: value,
        });
        return { name: envName, valueFrom: secret.arn };
    }

    function secretsManagerIamResource(secretArn: string): string {
        return secretArn.replace(/-[A-Za-z0-9]{6}$/, "-*");
    }

    function createMetricFilter(name: string, pattern: string, metricName: string): void {
        new aws.cloudwatch.LogMetricFilter(`${serviceName}-${name}-filter`, {
            name: `${serviceName}-${name}-filter`,
            logGroupName: logGroup.name,
            metricTransformation: {
                namespace: metricsNamespace,
                name: metricName,
                value: "1",
                defaultValue: "0",
            },
            pattern,
        }, { dependsOn: [logGroup, fargateTask] });
    }
};
