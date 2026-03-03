// CloudWatch Monitoring Stack
//
// Creates CloudWatch log groups, metric filters, alarms, and a dashboard for
// bare-metal nodes. Nodes already ship logs and metrics via Vector
// (ansible role: vector) — this stack owns the AWS-side monitoring resources.
//
// Alarm thresholds are tuned per-node in nodeConfig.ts following the same
// data-driven pattern as infra/indexer/alarmConfig.ts:
// - period is always explicit (no hidden defaults)
// - multi-severity escalation (SEV2 warning → SEV1 page)
// - descriptions map back to the actual evaluation math
//
// Usage:
//   cd infra/cw-monitoring
//   npm install
//   pulumi up -s staging     # or: pulumi up -s production

import * as aws from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import { resolveNodes, NodeConfigEntry, MonitoredNode, AlarmConfig, LogPatternAlarmConfig } from "./nodeConfig";
import { buildDashboardBody } from "./components/dashboard";

export = () => {
    const config = new pulumi.Config();
    const stackName = pulumi.getStack();

    // ── Load node inventory from stack config ────────────────────────────
    const rawNodes = config.requireObject<NodeConfigEntry[]>("nodes");
    const nodes = resolveNodes(rawNodes);

    // ── Configuration ────────────────────────────────────────────────────
    const metricsNamespace = config.get("metricsNamespace") || "Boundless/SystemMetrics";
    const logGroupPrefix = config.get("logGroupPrefix") || "/boundless/bento";
    const retentionDays = config.getNumber("logRetentionDays") || 30;
    const alarmNamespace = `Boundless/CWMonitoring/${stackName}`;

    // SNS topics — same pattern used by slasher, indexer, etc.
    const slackTopicArn = config.get("SLACK_ALERTS_TOPIC_ARN");
    const pagerdutyTopicArn = config.get("PAGERDUTY_ALERTS_TOPIC_ARN");
    const alarmActions = [slackTopicArn, pagerdutyTopicArn].filter(Boolean) as string[];

    // ── Per-node resources ───────────────────────────────────────────────
    for (const node of nodes) {
        createNodeMonitoring(node, {
            metricsNamespace,
            alarmNamespace,
            logGroupPrefix,
            retentionDays,
            alarmActions,
            stackName,
        });
    }

    // ── Dashboard ────────────────────────────────────────────────────────
    const dashboardBody = buildDashboardBody(nodes, metricsNamespace, alarmNamespace, logGroupPrefix);

    new aws.cloudwatch.Dashboard(`cw-${stackName}-dashboard`, {
        dashboardName: `cw-${stackName}`,
        dashboardBody,
    });

    // ── Outputs ──────────────────────────────────────────────────────────
    return {
        dashboardUrl: `https://us-west-2.console.aws.amazon.com/cloudwatch/home?region=us-west-2#dashboards/dashboard/cw-${stackName}`,
        nodeCount: nodes.length,
        logGroups: nodes.map(n => `${logGroupPrefix}/${n.hostname}`),
    };
};

// ── Node monitoring resources ────────────────────────────────────────────────

interface MonitoringOpts {
    metricsNamespace: string;
    alarmNamespace: string;
    logGroupPrefix: string;
    retentionDays: number;
    alarmActions: string[];
    stackName: string;
}

/**
 * Resolve AlarmConfig defaults so the spread satisfies MetricAlarmArgs required
 * fields. Follows the same pattern as indexer/components/alarms.ts —
 * comparisonOperator and treatMissingData get safe defaults.
 */
function withDefaults(ac: AlarmConfig) {
    return {
        ...ac.alarmConfig,
        comparisonOperator: ac.alarmConfig.comparisonOperator ?? "GreaterThanOrEqualToThreshold",
        treatMissingData: ac.alarmConfig.treatMissingData ?? "notBreaching",
    };
}

/** Short hash for alarm name uniqueness (same approach as indexer/components/alarms.ts). */
function descHash(s: string): string {
    return s.split("").reduce((acc, c) => ((acc << 5) - acc + c.charCodeAt(0)) & 0xfffffff, 5381)
        .toString(16).substring(0, 4);
}

function createNodeMonitoring(node: MonitoredNode, opts: MonitoringOpts): void {
    const { metricsNamespace, alarmNamespace, logGroupPrefix, retentionDays, alarmActions, stackName } = opts;
    const logGroupName = `${logGroupPrefix}/${node.hostname}`;
    const prefix = `cw-${stackName}-${node.name}`;

    // ── Log Group ────────────────────────────────────────────────────────
    // Vector creates the log group on first write. We adopt it into Pulumi
    // state so we can enforce retention and manage it as infra. The import
    // option tells Pulumi to import the existing resource rather than fail
    // with ResourceAlreadyExistsException.
    const logGroup = new aws.cloudwatch.LogGroup(`${prefix}-logs`, {
        name: logGroupName,
        retentionInDays: retentionDays,
    }, { import: logGroupName });

    // ── Alarms (data-driven from nodeConfig) ─────────────────────────────

    // 1. Bento service down
    for (const ac of node.alarms.bentoDown) {
        const h = descHash(ac.description);
        const a = withDefaults(ac);
        new aws.cloudwatch.MetricAlarm(`${prefix}-bento-down-${h}-${ac.severity}`, {
            name: `${prefix}-bento-down-${h}-${ac.severity}`,
            namespace: metricsNamespace,
            metricName: "bento_active",
            dimensions: { host: node.hostname },
            statistic: "Minimum",
            period: ac.metricConfig.period,
            ...a,
            alarmDescription: `${ac.severity}: ${ac.description} on ${node.name}`,
            actionsEnabled: true,
            alarmActions,
        });
    }

    // 2. No containers running
    for (const ac of node.alarms.noContainers) {
        const h = descHash(ac.description);
        const a = withDefaults(ac);
        new aws.cloudwatch.MetricAlarm(`${prefix}-no-containers-${h}-${ac.severity}`, {
            name: `${prefix}-no-containers-${h}-${ac.severity}`,
            namespace: metricsNamespace,
            metricName: "bento_containers",
            dimensions: { host: node.hostname },
            statistic: "Minimum",
            period: ac.metricConfig.period,
            ...a,
            alarmDescription: `${ac.severity}: ${ac.description} on ${node.name}`,
            actionsEnabled: true,
            alarmActions,
        });
    }

    // 3. Memory pressure (metric math)
    for (const ac of node.alarms.memoryHigh) {
        const h = descHash(ac.description);
        const a = withDefaults(ac);
        new aws.cloudwatch.MetricAlarm(`${prefix}-mem-${h}-${ac.severity}`, {
            name: `${prefix}-mem-${h}-${ac.severity}`,
            metricQueries: [
                {
                    id: "used",
                    metric: {
                        namespace: metricsNamespace,
                        metricName: "memory_used_bytes",
                        dimensions: { host: node.hostname },
                        period: ac.metricConfig.period,
                        stat: "Average",
                    },
                    returnData: false,
                },
                {
                    id: "total",
                    metric: {
                        namespace: metricsNamespace,
                        metricName: "memory_total_bytes",
                        dimensions: { host: node.hostname },
                        period: ac.metricConfig.period,
                        stat: "Average",
                    },
                    returnData: false,
                },
                {
                    id: "pct",
                    expression: "(used / total) * 100",
                    label: "Memory %",
                    returnData: true,
                },
            ],
            ...a,
            treatMissingData: a.treatMissingData ?? "missing",
            alarmDescription: `${ac.severity}: ${ac.description} on ${node.name}`,
            actionsEnabled: true,
            alarmActions,
        });
    }

    // 4. Disk pressure (metric math on root /)
    for (const ac of node.alarms.diskHigh) {
        const h = descHash(ac.description);
        const a = withDefaults(ac);
        new aws.cloudwatch.MetricAlarm(`${prefix}-disk-${h}-${ac.severity}`, {
            name: `${prefix}-disk-${h}-${ac.severity}`,
            metricQueries: [
                {
                    id: "used",
                    metric: {
                        namespace: metricsNamespace,
                        metricName: "filesystem_used_bytes",
                        dimensions: { host: node.hostname, mountpoint: "/" },
                        period: ac.metricConfig.period,
                        stat: "Average",
                    },
                    returnData: false,
                },
                {
                    id: "total",
                    metric: {
                        namespace: metricsNamespace,
                        metricName: "filesystem_total_bytes",
                        dimensions: { host: node.hostname, mountpoint: "/" },
                        period: ac.metricConfig.period,
                        stat: "Average",
                    },
                    returnData: false,
                },
                {
                    id: "pct",
                    expression: "(used / total) * 100",
                    label: "Disk %",
                    returnData: true,
                },
            ],
            ...a,
            treatMissingData: a.treatMissingData ?? "missing",
            alarmDescription: `${ac.severity}: ${ac.description} on ${node.name}`,
            actionsEnabled: true,
            alarmActions,
        });
    }

    // ── Log pattern alarms (broker error codes, etc.) ────────────────────
    // Each entry creates a log metric filter; entries with `alarm` also
    // create a CloudWatch alarm. Follows the same pattern as
    // createProverAlarms() from the ECS prover stack.

    for (const lp of node.alarms.logPatterns) {
        const severity = lp.alarm?.severity;
        const filterSuffix = severity ? `${lp.metricName}-${severity}` : lp.metricName;
        const filterName = `${prefix}-${filterSuffix}`;
        const metricFullName = `${node.name}-${filterSuffix}`;

        // Metric filter
        new aws.cloudwatch.LogMetricFilter(`${filterName}-filter`, {
            name: `${filterName}-filter`,
            logGroupName: logGroup.name,
            metricTransformation: {
                namespace: alarmNamespace,
                name: metricFullName,
                value: "1",
                defaultValue: "0",
            },
            pattern: lp.pattern,
        });

        // Alarm (optional — some patterns are metric-only for dashboards)
        if (lp.alarm) {
            const h = descHash(lp.alarm.description);
            new aws.cloudwatch.MetricAlarm(`${filterName}-${h}-alarm`, {
                name: `${filterName}-${h}`,
                metricQueries: [
                    {
                        id: "m1",
                        metric: {
                            namespace: alarmNamespace,
                            metricName: metricFullName,
                            period: lp.alarm.metricConfig.period,
                            stat: "Sum",
                        },
                        returnData: true,
                    },
                ],
                threshold: lp.alarm.alarmConfig.threshold,
                comparisonOperator: lp.alarm.alarmConfig.comparisonOperator ?? "GreaterThanOrEqualToThreshold",
                evaluationPeriods: lp.alarm.alarmConfig.evaluationPeriods,
                datapointsToAlarm: lp.alarm.alarmConfig.datapointsToAlarm,
                treatMissingData: lp.alarm.alarmConfig.treatMissingData ?? "notBreaching",
                alarmDescription: `${lp.alarm.severity}: ${lp.alarm.description} on ${node.name}`,
                actionsEnabled: true,
                alarmActions,
            });
        }
    }
}
