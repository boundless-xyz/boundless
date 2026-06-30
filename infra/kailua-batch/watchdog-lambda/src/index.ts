import {
    DescribeTasksCommand,
    ECSClient,
    ListTasksCommand,
    StopTaskCommand,
} from "@aws-sdk/client-ecs";
import type { Handler } from "aws-lambda";

export type KailuaBatchWatchdogResult = {
    scanned: number;
    killed: number;
    killedTaskArns: string[];
};

export const handler: Handler<unknown, KailuaBatchWatchdogResult> = async () => {
    const cluster = process.env.CLUSTER_ARN;
    const family = process.env.TASK_DEFINITION_FAMILY;
    if (!cluster || !family) {
        throw new Error("Missing required environment variable: CLUSTER_ARN or TASK_DEFINITION_FAMILY");
    }

    const maxAgeMinutes = parsePositiveInteger("TASK_KILL_AFTER_MINUTES", "30");
    const maxAgeMs = maxAgeMinutes * 60 * 1000;
    const ecs = new ECSClient({ region: process.env.AWS_REGION || "us-west-2" });

    const listed = await ecs.send(new ListTasksCommand({
        cluster,
        desiredStatus: "RUNNING",
        family,
    }));
    const taskArns = listed.taskArns ?? [];
    if (taskArns.length === 0) {
        console.log(JSON.stringify({
            msg: "KAILUA_WATCHDOG_SCAN",
            scanned: 0,
            killed: 0,
            maxAgeMinutes,
        }));
        return { scanned: 0, killed: 0, killedTaskArns: [] };
    }

    const killedTaskArns: string[] = [];
    for (const batch of chunk(taskArns, 100)) {
        const described = await ecs.send(new DescribeTasksCommand({
            cluster,
            tasks: batch,
        }));

        for (const task of described.tasks ?? []) {
            if (!task.taskArn || !task.startedAt) {
                continue;
            }

            const ageMs = Date.now() - task.startedAt.getTime();
            if (ageMs <= maxAgeMs) {
                continue;
            }

            const ageMinutes = Math.round(ageMs / 60_000);
            await ecs.send(new StopTaskCommand({
                cluster,
                task: task.taskArn,
                reason: `KAILUA_TASK_KILL exceeded ${maxAgeMinutes}m runtime (${ageMinutes}m)`,
            }));

            killedTaskArns.push(task.taskArn);
            console.log(JSON.stringify({
                msg: "KAILUA_TASK_KILLED",
                taskArn: task.taskArn,
                ageMinutes,
                maxAgeMinutes,
                startedAt: task.startedAt.toISOString(),
                capacityProvider: task.capacityProviderName,
                launchType: task.launchType,
            }));
        }
    }

    console.log(JSON.stringify({
        msg: "KAILUA_WATCHDOG_SCAN",
        scanned: taskArns.length,
        killed: killedTaskArns.length,
        maxAgeMinutes,
    }));

    return {
        scanned: taskArns.length,
        killed: killedTaskArns.length,
        killedTaskArns,
    };
};

function parsePositiveInteger(name: string, defaultValue: string): number {
    const value = parseInt(process.env[name] ?? defaultValue, 10);
    if (!Number.isFinite(value) || value < 1) {
        throw new Error(`Invalid ${name}: ${process.env[name]}`);
    }
    return value;
}

function chunk<T>(items: T[], size: number): T[][] {
    const batches: T[][] = [];
    for (let i = 0; i < items.length; i += size) {
        batches.push(items.slice(i, i + size));
    }
    return batches;
}
