// Latitude Monitoring Stack
//
// Creates CloudWatch log groups, metric filters, alarms, and a dashboard for
// Latitude bare-metal nodes. Nodes already ship logs and metrics via Vector
// (ansible role: vector) — this stack owns the AWS-side monitoring resources.
//
// Usage:
//   cd infra/latitude-monitoring
//   npm install
//   pulumi up -s staging     # or: pulumi up -s production

import * as aws from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import { Severity } from "../util";
import { staging, production, LatitudeNode } from "./nodeConfig";
import { buildDashboardBody } from "./components/dashboard";

export = () => {
    const config = new pulumi.Config();
    const stackName = pulumi.getStack();

    // ── Environment selection ────────────────────────────────────────────
    const envConfig = stackName.includes("staging") ? staging
        : stackName.includes("production") ? production
        : (() => { throw new Error(`Unknown stack "${stackName}": expected "staging" or "production" in name`); })();

    const nodes = envConfig.nodes;

    // ── Configuration ────────────────────────────────────────────────────
    const metricsNamespace = config.get("metricsNamespace") || "Boundless/SystemMetrics";
    const logGroupPrefix = config.get("logGroupPrefix") || "/boundless/bento";
    const retentionDays = config.getNumber("logRetentionDays") || 30;
    const alarmNamespace = `Boundless/Latitude/${stackName}`;

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

    new aws.cloudwatch.Dashboard(`latitude-${stackName}-dashboard`, {
        dashboardName: `latitude-${stackName}`,
        dashboardBody,
    });

    // ── Outputs ──────────────────────────────────────────────────────────
    return {
        dashboardUrl: `https://us-west-2.console.aws.amazon.com/cloudwatch/home?region=us-west-2#dashboards/dashboard/latitude-${stackName}`,
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

function createNodeMonitoring(node: LatitudeNode, opts: MonitoringOpts): void {
    const { metricsNamespace, alarmNamespace, logGroupPrefix, retentionDays, alarmActions, stackName } = opts;
    const logGroupName = `${logGroupPrefix}/${node.hostname}`;
    const prefix = `lat-${stackName}-${node.name}`;

    // ── Log Group ────────────────────────────────────────────────────────
    // Vector creates the log group on first write, but we want to own it in
    // Pulumi so we can enforce retention and track it as infrastructure.
    const logGroup = new aws.cloudwatch.LogGroup(`${prefix}-logs`, {
        name: logGroupName,
        retentionInDays: retentionDays,
    });

    // ── Log Metric Filters ───────────────────────────────────────────────
    // Count ERROR lines flowing through the log group (supplements the
    // bento_log_errors counter from Vector, which only counts journald errors).
    new aws.cloudwatch.LogMetricFilter(`${prefix}-error-filter`, {
        name: `${prefix}-log-errors`,
        logGroupName: logGroup.name,
        metricTransformation: {
            namespace: alarmNamespace,
            name: `${node.name}-log-errors`,
            value: "1",
            defaultValue: "0",
        },
        pattern: "ERROR",
    });

    new aws.cloudwatch.LogMetricFilter(`${prefix}-fatal-filter`, {
        name: `${prefix}-log-fatal`,
        logGroupName: logGroup.name,
        metricTransformation: {
            namespace: alarmNamespace,
            name: `${node.name}-log-fatal`,
            value: "1",
            defaultValue: "0",
        },
        pattern: "FATAL",
    });

    // ── Alarms ───────────────────────────────────────────────────────────

    // 1. Bento service down — Vector publishes bento_active gauge (0 or 1).
    //    If the metric is missing for 2 consecutive periods, treat as breaching
    //    (the node may have lost connectivity or Vector stopped).
    new aws.cloudwatch.MetricAlarm(`${prefix}-bento-down`, {
        name: `${prefix}-bento-down-${Severity.SEV1}`,
        namespace: metricsNamespace,
        metricName: "bento_active",
        dimensions: { host: node.hostname },
        statistic: "Minimum",
        period: 300,
        evaluationPeriods: 2,
        datapointsToAlarm: 2,
        threshold: 1,
        comparisonOperator: "LessThanThreshold",
        treatMissingData: "breaching",
        alarmDescription: `${Severity.SEV1}: Bento service down on ${node.name} (${node.ip})`,
        actionsEnabled: true,
        alarmActions,
    });

    // 2. No containers running — bento_containers = 0 means Docker Compose
    //    stack has no running containers despite the service being "active".
    new aws.cloudwatch.MetricAlarm(`${prefix}-no-containers`, {
        name: `${prefix}-no-containers-${Severity.SEV1}`,
        namespace: metricsNamespace,
        metricName: "bento_containers",
        dimensions: { host: node.hostname },
        statistic: "Minimum",
        period: 300,
        evaluationPeriods: 2,
        datapointsToAlarm: 2,
        threshold: 1,
        comparisonOperator: "LessThanThreshold",
        treatMissingData: "breaching",
        alarmDescription: `${Severity.SEV1}: No Bento containers running on ${node.name} (${node.ip})`,
        actionsEnabled: true,
        alarmActions,
    });

    // 3. High error rate — log metric filter errors exceeding threshold.
    new aws.cloudwatch.MetricAlarm(`${prefix}-high-errors`, {
        name: `${prefix}-high-errors-${Severity.SEV2}`,
        namespace: alarmNamespace,
        metricName: `${node.name}-log-errors`,
        statistic: "Sum",
        period: 300,
        evaluationPeriods: 3,
        datapointsToAlarm: 2,
        threshold: 20,
        comparisonOperator: "GreaterThanOrEqualToThreshold",
        treatMissingData: "notBreaching",
        alarmDescription: `${Severity.SEV2}: >=20 ERROR lines in 5 min on ${node.name} (${node.ip})`,
        actionsEnabled: true,
        alarmActions,
    });

    // 4. Fatal / crash — any FATAL log line is alarmed immediately.
    new aws.cloudwatch.MetricAlarm(`${prefix}-fatal`, {
        name: `${prefix}-fatal-${Severity.SEV1}`,
        namespace: alarmNamespace,
        metricName: `${node.name}-log-fatal`,
        statistic: "Sum",
        period: 60,
        evaluationPeriods: 1,
        datapointsToAlarm: 1,
        threshold: 1,
        comparisonOperator: "GreaterThanOrEqualToThreshold",
        treatMissingData: "notBreaching",
        alarmDescription: `${Severity.SEV1}: FATAL log on ${node.name} (${node.ip})`,
        actionsEnabled: true,
        alarmActions,
    });

    // 5. Memory pressure — metric math (used/total > 90%).
    new aws.cloudwatch.MetricAlarm(`${prefix}-memory-high`, {
        name: `${prefix}-memory-high-${Severity.SEV2}`,
        metricQueries: [
            {
                id: "used",
                metric: {
                    namespace: metricsNamespace,
                    metricName: "memory_used_bytes",
                    dimensions: { host: node.hostname },
                    period: 300,
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
                    period: 300,
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
        threshold: 90,
        comparisonOperator: "GreaterThanOrEqualToThreshold",
        evaluationPeriods: 3,
        datapointsToAlarm: 2,
        treatMissingData: "missing",
        alarmDescription: `${Severity.SEV2}: Memory usage >90% on ${node.name} (${node.ip})`,
        actionsEnabled: true,
        alarmActions,
    });

    // 6. Disk pressure — filesystem_used/total on root mountpoint > 85%.
    new aws.cloudwatch.MetricAlarm(`${prefix}-disk-high`, {
        name: `${prefix}-disk-high-${Severity.SEV2}`,
        metricQueries: [
            {
                id: "used",
                metric: {
                    namespace: metricsNamespace,
                    metricName: "filesystem_used_bytes",
                    dimensions: { host: node.hostname, mountpoint: "/" },
                    period: 300,
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
                    period: 300,
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
        threshold: 85,
        comparisonOperator: "GreaterThanOrEqualToThreshold",
        evaluationPeriods: 2,
        datapointsToAlarm: 2,
        treatMissingData: "missing",
        alarmDescription: `${Severity.SEV2}: Disk usage >85% on ${node.name} (${node.ip})`,
        actionsEnabled: true,
        alarmActions,
    });
}
