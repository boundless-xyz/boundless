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

const DEFAULT_SCHEDULE_COUNT = 20;
const DEFAULT_SCHEDULE_WINDOW_MINUTES = 20;
const DEFAULT_TASKS_PER_MINUTE = 2;
// Hard wall-clock cap on a kailua task; the watchdog in defaultProveScript SIGTERM/SIGKILLs
// the whole prove process group past this and the task exits.
const DEFAULT_TASK_TIMEOUT_MINUTES = 20;
// External ECS watchdog Lambda stops tasks still RUNNING past this age (StopTask → SIGKILL).
const DEFAULT_TASK_KILL_AFTER_MINUTES = 30;

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
    const scheduleCount = config.getNumber("SCHEDULE_COUNT") ?? DEFAULT_SCHEDULE_COUNT;
    const scheduleWindowMinutes = config.getNumber("SCHEDULE_WINDOW_MINUTES") ?? DEFAULT_SCHEDULE_WINDOW_MINUTES;
    const tasksPerMinute = config.getNumber("TASKS_PER_MINUTE") ?? DEFAULT_TASKS_PER_MINUTE;
    const taskTimeoutMinutes = config.getNumber("TASK_TIMEOUT_MINUTES") ?? DEFAULT_TASK_TIMEOUT_MINUTES;
    const taskKillAfterMinutes = config.getNumber("TASK_KILL_AFTER_MINUTES") ?? DEFAULT_TASK_KILL_AFTER_MINUTES;
    if (taskTimeoutMinutes < 1) {
        throw new Error("TASK_TIMEOUT_MINUTES must be at least 1");
    }
    if (taskKillAfterMinutes < 1) {
        throw new Error("TASK_KILL_AFTER_MINUTES must be at least 1");
    }
    if (scheduleCount < 1) {
        throw new Error("SCHEDULE_COUNT must be at least 1");
    }
    if (scheduleWindowMinutes < 1) {
        throw new Error("SCHEDULE_WINDOW_MINUTES must be at least 1");
    }
    if (tasksPerMinute < 1) {
        throw new Error("TASKS_PER_MINUTE must be at least 1");
    }
    if (scheduleCount > scheduleWindowMinutes) {
        throw new Error("SCHEDULE_COUNT must be less than or equal to SCHEDULE_WINDOW_MINUTES");
    }

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

    const deploymentOverrideKeys = [
        "BOUNDLESS_MARKET_ADDRESS",
        "BOUNDLESS_SET_VERIFIER_ADDRESS",
        "BOUNDLESS_COLLATERAL_TOKEN_ADDRESS",
    ] as const;
    const deploymentOverrides: Record<string, string> = {};
    for (const key of deploymentOverrideKeys) {
        const value = config.get(key);
        if (value) {
            deploymentOverrides[key] = value;
        }
    }
    if (
        Boolean(deploymentOverrides.BOUNDLESS_MARKET_ADDRESS) !==
        Boolean(deploymentOverrides.BOUNDLESS_SET_VERIFIER_ADDRESS)
    ) {
        throw new Error(
            "BOUNDLESS_MARKET_ADDRESS and BOUNDLESS_SET_VERIFIER_ADDRESS must be set together " +
            "(the kailua image requires both to build an explicit deployment; setting only one " +
            "silently falls back to the canonical production deployment)",
        );
    }

    const dynamicPricingTimeoutModifier = config.get("BOUNDLESS_DYNAMIC_PRICING_TIMEOUT_MODIFIER");
    const dynamicPricingRampUpModifier = config.get("BOUNDLESS_DYNAMIC_PRICING_RAMP_UP_MODIFIER");

    const defaultEnvironment: Record<string, string> = {
        MODE: config.get("MODE") ?? "debug",
        ...deploymentOverrides,
        BOUNDLESS_ORDER_SUBMISSION_COOLDOWN: "1",
        EXPORT_PROFILE_CSV: "true",
        BOUNDLESS_ORDER_FUNDING_MODE: config.get("BOUNDLESS_ORDER_FUNDING_MODE") ?? "available-balance",
        ENABLE_EXPERIMENTAL_WITNESS_ENDPOINT: "true",
        BOUNDLESS_LOOK_BACK: config.get("BOUNDLESS_LOOK_BACK") ?? "false",
        MAX_WITNESS_SIZE: "47185920",
        NO_COLOR: "1",
        R2_DOMAIN: config.require("R2_DOMAIN"),
        BOUNDLESS_DYNAMIC_PRICING: config.get("BOUNDLESS_DYNAMIC_PRICING") ?? "true",
        ...(dynamicPricingTimeoutModifier
            ? { BOUNDLESS_DYNAMIC_PRICING_TIMEOUT_MODIFIER: dynamicPricingTimeoutModifier }
            : {}),
        ...(dynamicPricingRampUpModifier
            ? { BOUNDLESS_DYNAMIC_PRICING_RAMP_UP_MODIFIER: dynamicPricingRampUpModifier }
            : {}),
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
        // Consumed by the prove watchdog in defaultProveScript; ENV stack config can override.
        KAILUA_TASK_TIMEOUT_SECONDS: (taskTimeoutMinutes * 60).toString(),
    };

    const orderStreamUrl = config.requireSecret("BOUNDLESS_ORDER_STREAM_URL");

    const extraEnvironment = config.getObject<Record<string, string>>("ENV") ?? {};
    const environment: { name: string; value: pulumi.Input<string> }[] = [
        ...Object.entries({ ...defaultEnvironment, ...extraEnvironment })
            .map(([name, value]) => ({ name, value })),
        { name: "BOUNDLESS_ORDER_STREAM_URL", value: orderStreamUrl },
    ];

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
        // Random start jitter so tasks launched together do not hit the RPC provider's
        // beacon/blob endpoints in lockstep. Runs before the finalized-block fetch so the
        // start block stays fresh, and before the prove watchdog so it does not consume the
        // proving budget. Cap must stay well under the watchdog lambda's TASK_KILL_AFTER_MINUTES.
        'KAILUA_START_JITTER=$((RANDOM % ${KAILUA_START_JITTER_MAX_SECONDS:-180}))',
        'echo "KAILUA_JITTER sleeping ${KAILUA_START_JITTER}s"',
        'sleep "$KAILUA_START_JITTER"',
        // Bound the pre-prove RPC call too: a dead/hung L2 RPC here would otherwise hang the
        // task forever, since the prove watchdog below has not started yet.
        'FINALIZED=$(timeout 60 cast bn finalized -r "$L2_RPC")',
        'START_BLOCK=$((FINALIZED - START_BLOCK_OFFSET))',
        'echo "KAILUA_RUNNING finalized=${FINALIZED} start_block=${START_BLOCK} offset=${START_BLOCK_OFFSET} block_count=${BLOCK_COUNT}"',
        `cd ${kailuaWorkdir}`,
        // Hard wall-clock cap on proving. A plain `timeout` does NOT bound this: `just`/the prover
        // don't propagate the signal to the whole process tree, so orphaned proving processes keep
        // running (observed: tasks ran ~52min under a 30min `timeout`, saturating MAX_RUNNING_TASKS).
        // Run prove in its own process group (set -m) and have a watchdog SIGTERM/SIGKILL the WHOLE
        // group on overrun, then exit so ECS tears the task down. Exit 111 stays the success/skip
        // code. The limit comes from TASK_TIMEOUT_MINUTES stack config (default 20m); override the
        // raw seconds via ENV stack config (KAILUA_TASK_TIMEOUT_SECONDS) if needed.
        'set -m',
        'just prove "$START_BLOCK" "$BLOCK_COUNT" "$L1_RPC" "$L1_BEACON" "$L2_RPC" "$OP_NODE" "$DATA_REL" "$MODE" "$SEQ_WINDOW" "$LOG_VERBOSITY" 1 &',
        'KAILUA_PROVE_PID=$!',
        '( sleep "${KAILUA_TASK_TIMEOUT_SECONDS:-1800}"; echo "KAILUA_TASK_TIMEOUT exceeded after ${KAILUA_TASK_TIMEOUT_SECONDS:-1800}s; killing prove process group"; kill -TERM -"$KAILUA_PROVE_PID" 2>/dev/null; sleep 30; kill -KILL -"$KAILUA_PROVE_PID" 2>/dev/null ) &',
        'KAILUA_WATCHDOG_PID=$!',
        'KAILUA_RC=0; wait "$KAILUA_PROVE_PID" || KAILUA_RC=$?',
        'kill "$KAILUA_WATCHDOG_PID" 2>/dev/null || true',
        'if [ "$KAILUA_RC" -eq 111 ]; then KAILUA_RC=0; fi',
        'exit "$KAILUA_RC"',
    ].join("\n");

    const containerCommand = config.get("KAILUA_COMMAND") ?? [
        "set -eu",
        defaultProveScript,
    ].join("\n");

    const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, { name: serviceName });

    const clusterCapacityProviders = new aws.ecs.ClusterCapacityProviders(
        `${serviceName}-capacity-providers`,
        {
            clusterName: cluster.name,
            capacityProviders: ["FARGATE", "FARGATE_SPOT"],
            defaultCapacityProviderStrategies: [
                {
                    capacityProvider: "FARGATE_SPOT",
                    weight: 1,
                    base: 0,
                },
            ],
        },
    );

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
                entryPoint: ["/bin/bash", "-c"],
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
                        Action: ["ecs:ListTasks", "ecs:DescribeTasks", "ecs:StopTask"],
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
    const taskCapacityProvider = config.get("TASK_CAPACITY_PROVIDER") ?? "FARGATE_SPOT";

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
                TASK_CAPACITY_PROVIDER: taskCapacityProvider,
                MAX_RUNNING_TASKS: maxRunningTasks.toString(),
                SCHEDULE_COUNT: scheduleCount.toString(),
                SCHEDULE_WINDOW_MINUTES: scheduleWindowMinutes.toString(),
                TASKS_PER_MINUTE: tasksPerMinute.toString(),
            },
        },
    }, { dependsOn: [triggerLambdaRole, fargateTask, triggerLambdaLogGroup, clusterCapacityProviders] });

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

    new aws.scheduler.Schedule(`${serviceName}-schedule`, {
        name: `${serviceName}-schedule`,
        description: `Every minute trigger; Lambda fans out ${tasksPerMinute} task(s) for ${scheduleCount}/${scheduleWindowMinutes} minute slots`,
        flexibleTimeWindow: {
            mode: "OFF",
        },
        scheduleExpression: "cron(* * * * ? *)",
        target: {
            arn: "arn:aws:scheduler:::aws-sdk:lambda:invoke",
            roleArn: schedulerRole.arn,
            input: triggerLambda.arn.apply(lambdaArn => JSON.stringify({
                FunctionName: lambdaArn,
                InvocationType: "Event",
                Payload: "{}",
            })),
        },
    }, { dependsOn: [triggerLambda] });

    const watchdogLambdaRole = new aws.iam.Role(`${serviceName}-watchdog-lambda-role`, {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
            Service: "lambda.amazonaws.com",
        }),
        managedPolicyArns: [aws.iam.ManagedPolicy.AWSLambdaBasicExecutionRole],
    });

    new aws.iam.RolePolicy(`${serviceName}-watchdog-lambda-policy`, {
        role: watchdogLambdaRole.id,
        policy: cluster.arn.apply(clusterArn => JSON.stringify({
            Version: "2012-10-17",
            Statement: [
                {
                    Effect: "Allow",
                    Action: ["ecs:ListTasks", "ecs:DescribeTasks", "ecs:StopTask"],
                    Resource: "*",
                    Condition: {
                        ArnEquals: {
                            "ecs:cluster": clusterArn,
                        },
                    },
                },
            ],
        })),
    });

    const watchdogLambdaLogGroup = new aws.cloudwatch.LogGroup(`${serviceName}-watchdog-logs`, {
        name: pulumi.interpolate`${serviceName}-watchdog`,
        retentionInDays: logRetentionDays,
    });

    const watchdogLambda = new aws.lambda.Function(`${serviceName}-watchdog`, {
        name: `${serviceName}-watchdog`,
        role: watchdogLambdaRole.arn,
        runtime: "nodejs20.x",
        handler: "index.handler",
        timeout: 60,
        loggingConfig: {
            logGroup: watchdogLambdaLogGroup.name,
            logFormat: "Text",
        },
        code: new pulumi.asset.AssetArchive({
            ".": new pulumi.asset.FileArchive("watchdog-lambda/build"),
        }),
        environment: {
            variables: {
                CLUSTER_ARN: cluster.arn,
                TASK_DEFINITION_FAMILY: serviceName,
                TASK_KILL_AFTER_MINUTES: taskKillAfterMinutes.toString(),
            },
        },
    }, { dependsOn: [watchdogLambdaRole, watchdogLambdaLogGroup] });

    const watchdogSchedulerRole = new aws.iam.Role(`${serviceName}-watchdog-scheduler-role`, {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
            Service: "scheduler.amazonaws.com",
        }),
    });

    new aws.iam.RolePolicy(`${serviceName}-watchdog-scheduler-policy`, {
        role: watchdogSchedulerRole.id,
        policy: watchdogLambda.arn.apply(lambdaArn => JSON.stringify({
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

    new aws.scheduler.Schedule(`${serviceName}-watchdog-schedule`, {
        name: `${serviceName}-watchdog-schedule`,
        description: `Every minute; stop kailua-batch tasks running longer than ${taskKillAfterMinutes} minutes`,
        flexibleTimeWindow: {
            mode: "OFF",
        },
        scheduleExpression: "cron(* * * * ? *)",
        target: {
            arn: "arn:aws:scheduler:::aws-sdk:lambda:invoke",
            roleArn: watchdogSchedulerRole.arn,
            input: watchdogLambda.arn.apply(lambdaArn => JSON.stringify({
                FunctionName: lambdaArn,
                InvocationType: "Event",
                Payload: "{}",
            })),
        },
    }, { dependsOn: [watchdogLambda] });

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

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-task-killed-filter`, {
        name: `${serviceName}-task-killed-filter`,
        logGroupName: watchdogLambdaLogGroup.name,
        metricTransformation: {
            namespace: metricsNamespace,
            name: `${serviceName}-task-killed`,
            value: "1",
            defaultValue: "0",
        },
        pattern: "KAILUA_TASK_KILLED",
    }, { dependsOn: [watchdogLambda, watchdogLambdaLogGroup] });

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
        scheduleCount,
        scheduleWindowMinutes,
        tasksPerMinute,
        scheduleExpression: "cron(* * * * ? *)",
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
