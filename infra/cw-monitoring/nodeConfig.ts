// Alarm presets and node types for CloudWatch monitoring.
//
// Node inventory (hostnames, roles) lives in the Pulumi stack config files
// (Pulumi.staging.yaml / Pulumi.production.yaml) — NOT in source code.
//
// Alarm tuning follows the same pattern as infra/indexer/alarmConfig.ts:
// - Each alarm category is an array: empty = disabled, multiple entries = multi-severity.
// - Profiles: "prod" = tight thresholds + pages, "nightly" = relaxed error thresholds,
//   "staging" = relaxed everything.

import { Severity } from "../util";
import * as aws from "@pulumi/aws";

// ── Types ────────────────────────────────────────────────────────────────────

export type AlarmConfig = {
    severity: Severity;
    description: string;
    metricConfig?: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric> & {
        period?: number;
    };
    alarmConfig: Partial<aws.cloudwatch.MetricAlarmArgs> & {
        evaluationPeriods: number;
        datapointsToAlarm: number;
        threshold: number;
        comparisonOperator?: string;
        treatMissingData?: string;
    };
};

export interface NodeAlarms {
    /** Bento systemd service is down (bento_active < 1). */
    bentoDown: AlarmConfig[];
    /** No Docker containers running (bento_containers < 1). */
    noContainers: AlarmConfig[];
    /** ERROR lines in CloudWatch log group (from log metric filter). */
    logErrors: AlarmConfig[];
    /** FATAL lines in CloudWatch log group. */
    logFatal: AlarmConfig[];
    /** Memory usage percentage (metric math: used/total). */
    memoryHigh: AlarmConfig[];
    /** Disk usage percentage on root / (metric math: used/total). */
    diskHigh: AlarmConfig[];
}

/** Raw node shape as defined in Pulumi stack config YAML. */
export interface NodeConfigEntry {
    name: string;
    hostname: string;
    role: "prover" | "explorer";
    chainId: string;
    /** Alarm profile key: "prod" | "nightly" | "staging" */
    alarmProfile: string;
}

/** Resolved node with alarm definitions attached. */
export interface MonitoredNode {
    /** Short human-readable name used in resource IDs and alarm names. */
    name: string;
    /** Machine hostname as reported by Vector (ansible_hostname). */
    hostname: string;
    /** Role: prover | explorer */
    role: "prover" | "explorer";
    /** Chain ID the node operates on. */
    chainId: string;
    /** Per-node alarm definitions. Empty arrays disable that alarm category. */
    alarms: NodeAlarms;
}

// ── Shared alarm presets ─────────────────────────────────────────────────────
// Reusable building blocks — override per-node as needed.

const PROD_BENTO_DOWN: AlarmConfig[] = [
    {
        severity: Severity.SEV1,
        description: "bento service down for 10 min",
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

const STAGING_BENTO_DOWN: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "bento service down for 30 min",
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

const PROD_NO_CONTAINERS: AlarmConfig[] = [
    {
        severity: Severity.SEV1,
        description: "no containers running for 10 min",
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

const STAGING_NO_CONTAINERS: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "no containers running for 30 min",
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

const PROD_LOG_ERRORS: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: ">=20 ERROR lines in 5 min, 2 of 3 periods",
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 2,
            threshold: 20,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
    {
        severity: Severity.SEV1,
        description: ">=50 ERROR lines in 5 min for 3 consecutive periods",
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 3,
            threshold: 50,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

// Staging is noisy during deploys — higher threshold, lower severity.
const STAGING_LOG_ERRORS: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: ">=50 ERROR lines in 5 min for 6 consecutive periods (30 min)",
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 50,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

const PROD_LOG_FATAL: AlarmConfig[] = [
    {
        severity: Severity.SEV1,
        description: "any FATAL log line",
        alarmConfig: {
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            threshold: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

const STAGING_LOG_FATAL: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "FATAL log twice in 1 hour",
        alarmConfig: {
            evaluationPeriods: 60,
            datapointsToAlarm: 2,
            threshold: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

const PROD_MEMORY_HIGH: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "memory >90% for 2 of 3 periods (15 min)",
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 2,
            threshold: 90,
        },
    },
];

const STAGING_MEMORY_HIGH: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "memory >95% for 3 consecutive periods (15 min)",
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 3,
            threshold: 95,
        },
    },
];

const PROD_DISK_HIGH: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "disk >85% for 2 consecutive periods",
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 85,
        },
    },
    {
        severity: Severity.SEV1,
        description: "disk >95% for 2 consecutive periods",
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 95,
        },
    },
];

const STAGING_DISK_HIGH: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "disk >90% for 2 consecutive periods",
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 90,
        },
    },
];

// ── Alarm profiles ───────────────────────────────────────────────────────────

const ALARM_PROFILES: Record<string, NodeAlarms> = {
    prod: {
        bentoDown: PROD_BENTO_DOWN,
        noContainers: PROD_NO_CONTAINERS,
        logErrors: PROD_LOG_ERRORS,
        logFatal: PROD_LOG_FATAL,
        memoryHigh: PROD_MEMORY_HIGH,
        diskHigh: PROD_DISK_HIGH,
    },
    staging: {
        bentoDown: STAGING_BENTO_DOWN,
        noContainers: STAGING_NO_CONTAINERS,
        logErrors: STAGING_LOG_ERRORS,
        logFatal: STAGING_LOG_FATAL,
        memoryHigh: STAGING_MEMORY_HIGH,
        diskHigh: STAGING_DISK_HIGH,
    },
    // Nightly builds break more often. Relax error thresholds but still alarm
    // on infra issues (service down, disk full).
    nightly: {
        bentoDown: PROD_BENTO_DOWN,
        noContainers: PROD_NO_CONTAINERS,
        logErrors: STAGING_LOG_ERRORS,  // relaxed — nightly is noisy
        logFatal: STAGING_LOG_FATAL,    // SEV2 not SEV1
        memoryHigh: PROD_MEMORY_HIGH,
        diskHigh: PROD_DISK_HIGH,
    },
};

// ── Public API ───────────────────────────────────────────────────────────────

/**
 * Resolve raw config entries into MonitoredNodes by attaching alarm presets.
 * Throws if an unknown alarmProfile is encountered.
 */
export function resolveNodes(entries: NodeConfigEntry[]): MonitoredNode[] {
    return entries.map(e => {
        const alarms = ALARM_PROFILES[e.alarmProfile];
        if (!alarms) {
            throw new Error(
                `Unknown alarmProfile "${e.alarmProfile}" for node "${e.name}". ` +
                `Valid profiles: ${Object.keys(ALARM_PROFILES).join(", ")}`,
            );
        }
        return {
            name: e.name,
            hostname: e.hostname,
            role: e.role,
            chainId: e.chainId,
            alarms,
        };
    });
}
