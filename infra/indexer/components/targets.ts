import * as aws from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import raw from "../targets.json";
import { Severity } from "../../util";

export interface AlarmConfig {
    severity: Severity,
    metricConfig: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric>,
    alarmConfig: Partial<aws.cloudwatch.MetricAlarmArgs>
}

export interface Target {
    name: string;
    address: string;
    submissionRate?: [AlarmConfig];
    successRate?: [AlarmConfig];
}

const stackName = pulumi.getStack();
const envKey = (stackName === "staging" || stackName === "dev") ? "staging" : "prod";

const envData = raw[envKey] as {
    clients: Target[];
    provers: Target[];
};

export const clients: Target[] = envData.clients;
export const provers: Target[] = envData.provers;

export const clientAddresses: string[] = clients.map(c => c.address);
export const proverAddresses: string[] = provers.map(p => p.address);
