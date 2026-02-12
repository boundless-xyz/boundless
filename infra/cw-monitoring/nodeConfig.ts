// Alarm presets and node types for CloudWatch monitoring.
//
// Node inventory (hostnames, roles) lives in the Pulumi stack config files
// (Pulumi.staging.yaml / Pulumi.production.yaml) — NOT in source code.
//
// Alarm tuning follows the same pattern as infra/indexer/alarmConfig.ts:
// - Each alarm category is an array: empty = disabled, multiple entries = multi-severity.
// - `period` is always required — no hidden defaults.
// - Every description maps back to the actual evaluation math.
// - Profiles: "prod" = tight thresholds + pages, "nightly" = relaxed error thresholds,
//   "staging" = relaxed everything.

import { Severity } from "../util";
import * as aws from "@pulumi/aws";

// ── Types ────────────────────────────────────────────────────────────────────

export type AlarmConfig = {
    severity: Severity;
    description: string;
    metricConfig: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric> & {
        /** Evaluation period in seconds. Required — no hidden defaults. */
        period: number;
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
// Reusable building blocks. Each alarm is self-documenting: description maps to
// the actual period × evaluationPeriods × datapointsToAlarm math.
//
// Multi-severity escalation: prod alarms use SEV2 as an early warning and SEV1
// as a page, matching the indexer pattern (e.g. Base mainnet og_offchain).

// ── Bento service down ──────────────────────────────────────────────────────
// Vector publishes bento_active gauge (0 or 1). Missing data = breaching
// because the node may have lost connectivity or Vector stopped.

const PROD_BENTO_DOWN: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "bento service down for 2 consecutive 5-min periods (10 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
    {
        severity: Severity.SEV1,
        description: "bento service down for 6 consecutive 5-min periods (30 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

// Staging deploys frequently and nodes restart — longer window, lower severity.
const STAGING_BENTO_DOWN: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "bento service down for 6 consecutive 5-min periods (30 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

// ── No containers running ───────────────────────────────────────────────────
// bento_containers = 0 means Docker Compose stack has no running containers
// despite the service being "active". Same escalation pattern as bentoDown.

const PROD_NO_CONTAINERS: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "no containers running for 2 consecutive 5-min periods (10 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
    {
        severity: Severity.SEV1,
        description: "no containers running for 6 consecutive 5-min periods (30 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

const STAGING_NO_CONTAINERS: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "no containers running for 6 consecutive 5-min periods (30 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            treatMissingData: "breaching",
        },
    },
];

// ── Log errors ──────────────────────────────────────────────────────────────
// Counts ERROR lines via CloudWatch log metric filter. notBreaching on missing
// data — no logs = no errors = OK.

const PROD_LOG_ERRORS: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: ">=20 ERROR lines per 5-min period, 2 of 3 periods (15 min window)",
        metricConfig: { period: 300 },
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
        description: ">=50 ERROR lines per 5-min period for 3 consecutive periods (15 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 3,
            threshold: 50,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

// Staging is noisy during deploys — higher threshold, longer window, SEV2 only.
const STAGING_LOG_ERRORS: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: ">=50 ERROR lines per 5-min period for 6 consecutive periods (30 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 6,
            datapointsToAlarm: 6,
            threshold: 50,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

// ── Fatal / crash ───────────────────────────────────────────────────────────
// Any FATAL log is a crash. Prod pages immediately; staging waits for a pattern.

const PROD_LOG_FATAL: AlarmConfig[] = [
    {
        severity: Severity.SEV1,
        description: "any FATAL log line in a 1-min period",
        metricConfig: { period: 60 },
        alarmConfig: {
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            threshold: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

// Staging nightlies crash during development — only alert if it's persistent.
const STAGING_LOG_FATAL: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: ">=1 FATAL log line in 2 of 12 consecutive 5-min periods (1 hour)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 12,
            datapointsToAlarm: 2,
            threshold: 1,
            comparisonOperator: "GreaterThanOrEqualToThreshold",
            treatMissingData: "notBreaching",
        },
    },
];

// ── Memory pressure ─────────────────────────────────────────────────────────
// Metric math: (memory_used_bytes / memory_total_bytes) * 100.
// Prover nodes routinely use high memory — 90% is a warning, 95% means OOM
// risk and should page.

const PROD_MEMORY_HIGH: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "memory >90% for 2 of 3 5-min periods (15 min window)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 2,
            threshold: 90,
        },
    },
    {
        severity: Severity.SEV1,
        description: "memory >95% for 3 consecutive 5-min periods (15 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 3,
            threshold: 95,
        },
    },
];

// Staging can run hotter without paging.
const STAGING_MEMORY_HIGH: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "memory >95% for 3 consecutive 5-min periods (15 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 3,
            datapointsToAlarm: 3,
            threshold: 95,
        },
    },
];

// ── Disk pressure ───────────────────────────────────────────────────────────
// Metric math: (filesystem_used_bytes / filesystem_total_bytes) * 100 on root.
// Disk fills slowly so 10-min windows are fine. 85% is a heads-up (time to
// clean up), 95% is critical (node will stall).

const PROD_DISK_HIGH: AlarmConfig[] = [
    {
        severity: Severity.SEV2,
        description: "disk >85% for 2 consecutive 5-min periods (10 min)",
        metricConfig: { period: 300 },
        alarmConfig: {
            evaluationPeriods: 2,
            datapointsToAlarm: 2,
            threshold: 85,
        },
    },
    {
        severity: Severity.SEV1,
        description: "disk >95% for 2 consecutive 5-min periods (10 min)",
        metricConfig: { period: 300 },
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
        description: "disk >90% for 2 consecutive 5-min periods (10 min)",
        metricConfig: { period: 300 },
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
    // Nightly builds break more often. Relax error/fatal thresholds but still
    // page on infra issues (service down, disk full, OOM).
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
