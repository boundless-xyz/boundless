import {
    ECSClient,
    ListTasksCommand,
    RunTaskCommand,
    type RunTaskCommandInput,
} from "@aws-sdk/client-ecs";
import type { Handler } from "aws-lambda";

export type KailuaBatchTriggerEvent = {
    startBlockOffset: number;
    blockCount: string;
};

export type KailuaBatchTriggerResult = {
    started: boolean;
    taskArn?: string;
    runningCount?: number;
    reason?: string;
};

export const handler: Handler<KailuaBatchTriggerEvent, KailuaBatchTriggerResult> = async event => {
    const required = [
        "CLUSTER_ARN",
        "TASK_DEFINITION_FAMILY",
        "TASK_DEFINITION_ARN",
        "CONTAINER_NAME",
        "SUBNET_IDS",
        "SECURITY_GROUP_ID",
        "ASSIGN_PUBLIC_IP",
    ] as const;
    for (const key of required) {
        if (!process.env[key]) {
            throw new Error(`Missing required environment variable: ${key}`);
        }
    }

    const maxRunning = parseInt(process.env.MAX_RUNNING_TASKS ?? "3", 10);
    if (!Number.isFinite(maxRunning) || maxRunning < 1) {
        throw new Error(`Invalid MAX_RUNNING_TASKS: ${process.env.MAX_RUNNING_TASKS}`);
    }

    const startBlockOffset = event.startBlockOffset;
    const blockCount = event.blockCount;
    if (startBlockOffset === undefined || blockCount === undefined) {
        throw new Error("Event must include startBlockOffset and blockCount");
    }

    const ecs = new ECSClient({ region: process.env.AWS_REGION || "us-west-2" });
    const cluster = process.env.CLUSTER_ARN!;
    const family = process.env.TASK_DEFINITION_FAMILY!;

    const listed = await ecs.send(new ListTasksCommand({
        cluster,
        desiredStatus: "RUNNING",
        family,
    }));
    const runningCount = listed.taskArns?.length ?? 0;

    if (runningCount >= maxRunning) {
        console.log(JSON.stringify({
            msg: "KAILUA_TASK_SKIPPED",
            reason: "max_running_tasks",
            runningCount,
            maxRunning,
            startBlockOffset,
            blockCount,
        }));
        return {
            started: false,
            runningCount,
            reason: `already ${runningCount} running (max ${maxRunning})`,
        };
    }

    const subnets = process.env.SUBNET_IDS!.split(",");
    const runInput: RunTaskCommandInput = {
        cluster,
        taskDefinition: process.env.TASK_DEFINITION_ARN!,
        launchType: "FARGATE",
        networkConfiguration: {
            awsvpcConfiguration: {
                subnets,
                securityGroups: [process.env.SECURITY_GROUP_ID!],
                assignPublicIp: process.env.ASSIGN_PUBLIC_IP === "ENABLED" ? "ENABLED" : "DISABLED",
            },
        },
        overrides: {
            containerOverrides: [
                {
                    name: process.env.CONTAINER_NAME!,
                    environment: [
                        { name: "BLOCK_COUNT", value: blockCount },
                        { name: "START_BLOCK_OFFSET", value: String(startBlockOffset) },
                    ],
                },
            ],
        },
    };

    const response = await ecs.send(new RunTaskCommand(runInput));
    const taskArn = response.tasks?.[0]?.taskArn;
    if (!taskArn) {
        const failures = response.failures?.map(f => `${f.reason}: ${f.arn ?? "unknown"}`).join("; ");
        throw new Error(`RunTask returned no task ARN${failures ? ` (${failures})` : ""}`);
    }

    console.log(JSON.stringify({
        msg: "KAILUA_TASK_STARTED",
        taskArn,
        runningCount,
        maxRunning,
        startBlockOffset,
        blockCount,
    }));

    return { started: true, taskArn, runningCount };
};
