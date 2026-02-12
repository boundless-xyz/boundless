import { MonitoredNode } from "../nodeConfig";
import { Severity } from "../../util";

// CloudWatch dashboard JSON builder for monitored nodes.
// Produces a grid layout: one row of system panels per node, plus a summary row.

const REGION = "us-west-2";

interface Widget {
    type: string;
    x: number;
    y: number;
    width: number;
    height: number;
    properties: Record<string, unknown>;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function metricWidget(
    title: string,
    metrics: unknown[][],
    x: number,
    y: number,
    width: number,
    height: number,
    opts?: { yAxisLabel?: string; stat?: string; period?: number; view?: string },
): Widget {
    return {
        type: "metric",
        x, y, width, height,
        properties: {
            title,
            region: REGION,
            metrics,
            stat: opts?.stat ?? "Average",
            period: opts?.period ?? 60,
            view: opts?.view ?? "timeSeries",
            yAxis: opts?.yAxisLabel ? { left: { label: opts.yAxisLabel } } : undefined,
        },
    };
}

function textWidget(markdown: string, x: number, y: number, width: number, height: number): Widget {
    return {
        type: "text",
        x, y, width, height,
        properties: { markdown },
    };
}

function logWidget(
    title: string,
    logGroupName: string,
    query: string,
    x: number,
    y: number,
    width: number,
    height: number,
): Widget {
    return {
        type: "log",
        x, y, width, height,
        properties: {
            title,
            region: REGION,
            query: `SOURCE '${logGroupName}' | ${query}`,
            view: "table",
        },
    };
}

// ── Per-node row (24 columns wide, 6 units tall) ────────────────────────────

function nodeRow(
    node: MonitoredNode,
    metricsNamespace: string,
    alarmNamespace: string,
    logGroupPrefix: string,
    yOffset: number,
): Widget[] {
    const host = node.hostname;
    const logGroup = `${logGroupPrefix}/${host}`;
    const dim = { host };

    const ROW_H = 6;
    const widgets: Widget[] = [];

    // Header
    widgets.push(textWidget(
        `### ${node.name}\n${node.role} | chain ${node.chainId}`,
        0, yOffset, 24, 1,
    ));
    const y = yOffset + 1;

    // Bento status (active gauge + container count)
    widgets.push(metricWidget(
        "Bento Status",
        [
            [metricsNamespace, "bento_active", "host", host, { label: "Active (0/1)" }],
            [metricsNamespace, "bento_containers", "host", host, { label: "Containers" }],
        ],
        0, y, 6, ROW_H,
        { stat: "Maximum", yAxisLabel: "count" },
    ));

    // Memory usage
    widgets.push(metricWidget(
        "Memory",
        [
            [metricsNamespace, "memory_used_bytes", "host", host, { label: "Used" }],
            [metricsNamespace, "memory_total_bytes", "host", host, { label: "Total" }],
        ],
        6, y, 6, ROW_H,
        { yAxisLabel: "bytes" },
    ));

    // Filesystem usage (root mountpoint)
    widgets.push(metricWidget(
        "Disk (root /)",
        [
            [metricsNamespace, "filesystem_used_bytes", "host", host, "mountpoint", "/", { label: "Used" }],
            [metricsNamespace, "filesystem_total_bytes", "host", host, "mountpoint", "/", { label: "Total" }],
        ],
        12, y, 6, ROW_H,
        { yAxisLabel: "bytes" },
    ));

    // Log errors (Vector bento_log_errors + prover unexpected-errors from log patterns)
    widgets.push(metricWidget(
        "Log Errors",
        [
            [metricsNamespace, "bento_log_errors", "host", host, { label: "Errors (Vector)", stat: "Sum" }],
            [alarmNamespace, `${node.name}-unexpected-errors-${Severity.SEV2}`, { label: "Broker 500s", stat: "Sum" }],
        ],
        18, y, 6, ROW_H,
        { stat: "Sum", period: 300, yAxisLabel: "count" },
    ));

    return widgets;
}

// ── Summary row ──────────────────────────────────────────────────────────────

function summaryRow(
    nodes: MonitoredNode[],
    metricsNamespace: string,
    yOffset: number,
): Widget[] {
    const widgets: Widget[] = [];
    const ROW_H = 6;

    widgets.push(textWidget("## Fleet Overview", 0, yOffset, 24, 1));
    const y = yOffset + 1;

    // All nodes bento_active overlay
    const bentoMetrics = nodes.map(n => [
        metricsNamespace, "bento_active", "host", n.hostname, { label: n.name },
    ]);
    widgets.push(metricWidget("Bento Active (all nodes)", bentoMetrics, 0, y, 8, ROW_H, { stat: "Minimum" }));

    // All nodes memory
    const memMetrics = nodes.map(n => [
        metricsNamespace, "memory_used_bytes", "host", n.hostname, { label: n.name },
    ]);
    widgets.push(metricWidget("Memory Used (all nodes)", memMetrics, 8, y, 8, ROW_H, { yAxisLabel: "bytes" }));

    // All nodes log errors
    const errMetrics = nodes.map(n => [
        metricsNamespace, "bento_log_errors", "host", n.hostname, { label: n.name },
    ]);
    widgets.push(metricWidget("Log Errors (all nodes)", errMetrics, 16, y, 8, ROW_H, { stat: "Sum", period: 300 }));

    return widgets;
}

// ── Public API ───────────────────────────────────────────────────────────────

export function buildDashboardBody(
    nodes: MonitoredNode[],
    metricsNamespace: string,
    alarmNamespace: string,
    logGroupPrefix: string,
): string {
    const widgets: Widget[] = [];

    // Summary at top
    const summaryWidgets = summaryRow(nodes, metricsNamespace, 0);
    widgets.push(...summaryWidgets);

    // Per-node rows (header 1 + row 6 = 7 units per node)
    const NODE_ROW_HEIGHT = 8; // 1 header + 6 chart + 1 gap
    let yOffset = 8; // after summary (1 header + 6 chart + 1 gap)

    for (const node of nodes) {
        widgets.push(...nodeRow(node, metricsNamespace, alarmNamespace, logGroupPrefix, yOffset));
        yOffset += NODE_ROW_HEIGHT;
    }

    return JSON.stringify({ widgets });
}
