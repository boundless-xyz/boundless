import {
    ECSClient,
    ListTasksCommand,
    RunTaskCommand,
    type RunTaskCommandInput,
} from "@aws-sdk/client-ecs";
import type { Handler } from "aws-lambda";

export type KailuaBatchTriggerEvent = {
    startBlockOffset?: number;
    blockCount?: string;
    scheduledAt?: string;
};

export type KailuaBatchTriggerResult = {
    started: boolean;
    taskArn?: string;
    taskArns?: string[];
    runningCount?: number;
    startedCount?: number;
    skippedCount?: number;
    reason?: string;
};

type TaskRequest = {
    startBlockOffset: number;
    blockCount: string;
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

    const maxRunning = parsePositiveInteger("MAX_RUNNING_TASKS", "3");
    const taskRequests = buildTaskRequests(event);
    if (taskRequests.length === 0) {
        return {
            started: false,
            startedCount: 0,
            skippedCount: 0,
            reason: "inactive minute slot",
        };
    }

    const ecs = new ECSClient({ region: process.env.AWS_REGION || "us-west-2" });
    const cluster = process.env.CLUSTER_ARN!;
    const family = process.env.TASK_DEFINITION_FAMILY!;

    const listed = await ecs.send(new ListTasksCommand({
        cluster,
        desiredStatus: "RUNNING",
        family,
    }));
    let runningCount = listed.taskArns?.length ?? 0;

    const taskArns: string[] = [];
    let skippedCount = 0;
    for (const { startBlockOffset, blockCount } of taskRequests) {
        if (runningCount >= maxRunning) {
            skippedCount++;
            console.log(JSON.stringify({
                msg: "KAILUA_TASK_SKIPPED",
                reason: "max_running_tasks",
                runningCount,
                maxRunning,
                startBlockOffset,
                blockCount,
            }));
            continue;
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

        taskArns.push(taskArn);
        runningCount++;

        console.log(JSON.stringify({
            msg: "KAILUA_TASK_STARTED",
            taskArn,
            runningCount,
            maxRunning,
            startBlockOffset,
            blockCount,
        }));
    }

    return {
        started: taskArns.length > 0,
        taskArn: taskArns[0],
        taskArns,
        runningCount,
        startedCount: taskArns.length,
        skippedCount,
        reason: taskArns.length === 0 ? `already ${runningCount} running (max ${maxRunning})` : undefined,
    };
};

function buildTaskRequests(event: KailuaBatchTriggerEvent): TaskRequest[] {
    if (event.startBlockOffset !== undefined || event.blockCount !== undefined) {
        if (event.startBlockOffset === undefined || event.blockCount === undefined) {
            throw new Error("Event must include both startBlockOffset and blockCount");
        }
        return [{ startBlockOffset: event.startBlockOffset, blockCount: event.blockCount }];
    }

    const scheduleCount = parsePositiveInteger("SCHEDULE_COUNT", "20");
    const scheduleWindowMinutes = parsePositiveInteger("SCHEDULE_WINDOW_MINUTES", "20");
    const tasksPerMinute = parsePositiveInteger("TASKS_PER_MINUTE", "2");
    if (scheduleCount > scheduleWindowMinutes) {
        throw new Error("SCHEDULE_COUNT must be less than or equal to SCHEDULE_WINDOW_MINUTES");
    }

    const now = event.scheduledAt ? new Date(event.scheduledAt) : new Date();
    if (Number.isNaN(now.getTime())) {
        throw new Error(`Invalid scheduledAt: ${event.scheduledAt}`);
    }

    const minuteSlot = now.getUTCMinutes() % scheduleWindowMinutes;
    if (minuteSlot >= scheduleCount) {
        console.log(JSON.stringify({
            msg: "KAILUA_SCHEDULE_SKIPPED",
            reason: "inactive_minute_slot",
            minuteSlot,
            scheduleCount,
            scheduleWindowMinutes,
        }));
        return [];
    }

    const taskRequests: TaskRequest[] = [];
    for (let taskIndex = 0; taskIndex < tasksPerMinute; taskIndex++) {
        taskRequests.push({
            startBlockOffset: minuteSlot * tasksPerMinute + taskIndex,
            blockCount: String(taskIndex + 1),
        });
    }
    return taskRequests;
}

function parsePositiveInteger(name: string, defaultValue: string): number {
    const value = parseInt(process.env[name] ?? defaultValue, 10);
    if (!Number.isFinite(value) || value < 1) {
        throw new Error(`Invalid ${name}: ${process.env[name]}`);
    }
    return value;
}
