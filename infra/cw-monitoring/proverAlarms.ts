// Prover (broker) log-pattern alarm definitions.
//
// Translated from the ECS-based createProverAlarms() into the cw-monitoring
// data-driven model. Each entry is a CloudWatch Logs filter pattern paired
// with an optional alarm. When `alarm` is undefined the entry creates a metric
// filter only (tracking without alerting).
//
// Thresholds vary by proverType (BENTO vs BONSAI) and chainId (Sepolia has
// higher thresholds for some tx-confirmation alarms).

import { Severity, ChainId } from "../util";
import { LogPatternAlarmConfig } from "./nodeConfig";

export type ProverType = "bento" | "bonsai";

/**
 * Build the full set of prover log-pattern alarm configs for a node.
 * Returns an empty array for non-prover nodes.
 */
export function buildProverLogPatterns(
    proverType: ProverType,
    chainId: string,
): LogPatternAlarmConfig[] {
    // ── Threshold presets by prover type ──────────────────────────────────
    // Bonsai prover is nearing end of life and "db locked" is a known issue
    // we won't fix. Raising thresholds to reduce noise.
    const isBento = proverType === "bento";
    const brokerUnexpectedThreshold = isBento ? 5 : 25;
    const supervisorUnexpectedThreshold = isBento ? 5 : 25;
    const svcUnexpectedThreshold = isBento ? 2 : 15;
    const svcUnexpectedThresholdHigh = isBento ? 3 : 15;
    const dbLockedThreshold = isBento ? 1 : 10;
    const isSepolia = chainId === ChainId.ETH_SEPOLIA || chainId === ChainId.BASE_SEPOLIA;

    return [
        // ── Broker-wide alarms ───────────────────────────────────────────
        // NOTE: AWS has a limit of 5 filter patterns containing regex per log group.
        // https://docs.aws.amazon.com/AmazonCloudWatch/latest/logs/FilterAndPattern.html

        // [Regex] Unexpected errors (500) across entire broker in 5 min
        {
            pattern: '%\\[B-[A-Z]+-500\\]%',
            metricName: "unexpected-errors",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${brokerUnexpectedThreshold} unexpected errors (500) across broker in 5 min`,
                metricConfig: { period: 300 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: brokerUnexpectedThreshold,
                },
            },
        },

        // [Regex] 10 errors of any kind across broker in 1 hour
        {
            pattern: '%\\[B-[A-Z]+-\\d+\\]%',
            metricName: "assorted-errors",
            alarm: {
                severity: Severity.SEV2,
                description: ">=10 assorted broker errors in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: 10,
                },
            },
        },

        // Any ERROR log without a structured error code (catch-all).
        // Excludes INTERNAL_ERROR which is returned by dependencies on retryable errors.
        {
            pattern: 'ERROR -"[B-" -"INTERNAL_ERROR"',
            metricName: "error-without-code",
            alarm: {
                severity: Severity.SEV2,
                description: "ERROR log without structured error code",
                metricConfig: { period: 60 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: 1,
                },
            },
        },

        // ── Balance alarms ───────────────────────────────────────────────
        // Once breached, the log continues on every tx, so we use a 1-hour
        // period to prevent noise from repeated triggers.
        {
            pattern: 'WARN "[B-BAL-ETH]"',
            metricName: "low-balance-alert-eth",
            alarm: {
                severity: Severity.SEV2,
                description: "low ETH balance warning in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 1 },
            },
        },
        {
            pattern: 'WARN "[B-BAL-STK]"',
            metricName: "low-balance-alert-stk",
            alarm: {
                severity: Severity.SEV2,
                description: "low stake balance warning in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 1 },
            },
        },
        {
            pattern: 'ERROR "[B-BAL-ETH]"',
            metricName: "low-balance-alert-eth",
            alarm: {
                severity: Severity.SEV2,
                description: "critical ETH balance error in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 1 },
            },
        },
        {
            pattern: 'ERROR "[B-BAL-STK]"',
            metricName: "low-balance-alert-stk",
            alarm: {
                severity: Severity.SEV2,
                description: "critical stake balance error in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 1 },
            },
        },

        // ── Supervisor alarms ────────────────────────────────────────────
        {
            pattern: '"[B-SUP-RECOVER]"',
            metricName: "supervisor-recover-errors",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${supervisorUnexpectedThreshold} supervisor restarts in 15 min`,
                metricConfig: { period: 900 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: supervisorUnexpectedThreshold,
                },
            },
        },
        {
            pattern: '"[B-SUP-FAULT]"',
            metricName: "supervisor-fault-errors",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 supervisor faults in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },

        // ── DB alarms ────────────────────────────────────────────────────
        {
            pattern: '"[B-DB-001]"',
            metricName: "db-locked-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${dbLockedThreshold} DB locked errors in 30 min`,
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: dbLockedThreshold },
            },
        },
        {
            pattern: '"[B-DB-002]"',
            metricName: "db-pool-timeout-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${dbLockedThreshold} DB pool timeout errors in 30 min`,
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: dbLockedThreshold },
            },
        },
        {
            pattern: '"[B-DB-500]"',
            metricName: "db-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${dbLockedThreshold} DB unexpected errors in 30 min`,
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: dbLockedThreshold },
            },
        },

        // ── Storage alarms ───────────────────────────────────────────────
        {
            pattern: '"[B-STR-002]"',
            metricName: "storage-http-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 storage HTTP errors (rate limiting, etc.) in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },
        {
            pattern: '"[B-STR-500]"',
            metricName: "storage-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 unexpected storage errors in 1 min",
                metricConfig: { period: 60 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },

        // ── Market Monitor alarms ────────────────────────────────────────
        {
            pattern: '"[B-MM-501]"',
            metricName: "market-monitor-event-polling-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 market monitor event polling errors in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },
        {
            pattern: '"[B-MM-501]"',
            metricName: "market-monitor-event-polling-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=10 market monitor event polling errors in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 10 },
            },
        },
        {
            pattern: '"[B-MM-502]"',
            metricName: "market-monitor-log-processing-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=5 market monitor log processing errors in 15 min",
                metricConfig: { period: 900 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 5 },
            },
        },
        {
            pattern: '"[B-MM-500]"',
            metricName: "market-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 market monitor unexpected errors in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        {
            pattern: '"[B-MM-500]"',
            metricName: "market-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 market monitor unexpected errors in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },

        // ── Chain Monitor alarms ─────────────────────────────────────────
        // RPC errors can occur transiently.
        {
            pattern: '"[B-CHM-400]"',
            metricName: "chain-monitor-rpc-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=5 chain monitor RPC errors in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 5 },
            },
        },
        {
            pattern: '"[B-CHM-500]"',
            metricName: "chain-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 chain monitor unexpected errors in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        {
            pattern: '"[B-CHM-500]"',
            metricName: "chain-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 chain monitor unexpected errors in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },

        // ── Off-chain Market Monitor alarms ──────────────────────────────
        {
            pattern: '"[B-OMM-001]"',
            metricName: "off-chain-market-monitor-websocket-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 off-chain market monitor websocket errors in 15 min",
                metricConfig: { period: 900 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },
        {
            pattern: '"[B-OMM-001]"',
            metricName: "off-chain-market-monitor-websocket-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=10 off-chain market monitor websocket errors in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 10 },
            },
        },
        {
            pattern: '"[B-OMM-500]"',
            metricName: "off-chain-market-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 off-chain market monitor unexpected errors in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        {
            pattern: '"[B-OMM-500]"',
            metricName: "off-chain-market-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 off-chain market monitor unexpected errors in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },

        // ── Order Picker alarms ──────────────────────────────────────────
        {
            pattern: '"[B-OP-500]"',
            metricName: "order-picker-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 order picker unexpected errors in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        {
            pattern: '"[B-OP-500]"',
            metricName: "order-picker-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 order picker unexpected errors in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },
        // Metric only — errors fetching images/inputs may be user error.
        {
            pattern: '?"[B-OP-001]" ?"[B-OP-002]"',
            metricName: "order-picker-fetch-error",
        },
        {
            pattern: '"[B-OP-005]"',
            metricName: "order-picker-rpc-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 order picker RPC errors in 15 min",
                metricConfig: { period: 900 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },

        // ── Order Monitor alarms ─────────────────────────────────────────
        {
            pattern: '"[B-OM-500]"',
            metricName: "order-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 order monitor unexpected errors in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        {
            pattern: '"[B-OM-500]"',
            metricName: "order-monitor-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=4 order monitor unexpected errors in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 4 },
            },
        },
        // Metric only — expected when another prover locks before us.
        { pattern: '"[B-OM-007]"', metricName: "order-monitor-lock-tx-failed" },
        { pattern: '"[B-OM-009]"', metricName: "order-monitor-already-locked" },
        {
            pattern: '"[B-OM-010]"',
            metricName: "order-monitor-insufficient-balance",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 insufficient balance for lock in 2 hours",
                metricConfig: { period: 7200 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        // Sepolia sees more tx-not-confirmed errors than other chains.
        {
            pattern: '"[B-OM-006]"',
            metricName: "order-monitor-lock-tx-not-confirmed",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${isSepolia ? 10 : 3} lock tx not confirmed in 1 hour`,
                metricConfig: { period: 3600 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: isSepolia ? 10 : 3,
                },
            },
        },
        {
            pattern: '"[B-OM-011]"',
            metricName: "order-monitor-rpc-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 order monitor RPC errors in 15 min",
                metricConfig: { period: 900 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },

        // ── Prover alarms ────────────────────────────────────────────────
        {
            pattern: '"[B-PRO-500]"',
            metricName: "prover-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${svcUnexpectedThreshold} prover unexpected errors in 30 min`,
                metricConfig: { period: 1800 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: svcUnexpectedThreshold,
                },
            },
        },
        {
            pattern: '"[B-PRO-500]"',
            metricName: "prover-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${svcUnexpectedThresholdHigh} prover unexpected errors in 5 min`,
                metricConfig: { period: 300 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: svcUnexpectedThresholdHigh,
                },
            },
        },
        {
            pattern: '"[B-PRO-501]"',
            metricName: "prover-proving-failed",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 proving-with-retries failures in 30 min",
                metricConfig: { period: 1800 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },

        // ── Aggregator alarms ────────────────────────────────────────────
        // Compression failure indicates a fault with the prover.
        {
            pattern: '"[B-AGG-400]"',
            metricName: "aggregator-compression-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 batch compression failures in 2 hours",
                metricConfig: { period: 7200 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        {
            pattern: '"[B-AGG-500]"',
            metricName: "aggregator-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${svcUnexpectedThreshold} aggregator unexpected errors in 30 min`,
                metricConfig: { period: 1800 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: svcUnexpectedThreshold,
                },
            },
        },
        {
            pattern: '"[B-AGG-500]"',
            metricName: "aggregator-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${svcUnexpectedThresholdHigh} aggregator unexpected errors in 5 min`,
                metricConfig: { period: 300 },
                alarmConfig: {
                    evaluationPeriods: 1,
                    datapointsToAlarm: 1,
                    threshold: svcUnexpectedThresholdHigh,
                },
            },
        },
        // Edge case: order expired in aggregator → indicates a slashed order.
        {
            pattern: '"[B-AGG-600]"',
            metricName: "aggregator-order-expired",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 aggregator order expirations in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },

        // ── Proving engine ───────────────────────────────────────────────
        // Metric only — internal errors are expected occasionally, retried,
        // and covered by other alarms.
        { pattern: '"[B-BON-008]"', metricName: "proving-engine-internal-error" },

        // ── Submitter alarms ─────────────────────────────────────────────
        // All-requests-expired gets logged multiple times per batch due to
        // retries, so threshold is higher.
        {
            pattern: '"[B-SUB-001]"',
            metricName: "submitter-requests-expired-before-submission",
            alarm: {
                severity: Severity.SEV2,
                description: ">=4 batch-all-expired events in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 4 },
            },
        },
        {
            pattern: '"[B-SUB-005]"',
            metricName: "submitter-some-requests-expired-before-submission",
            alarm: {
                severity: Severity.SEV2,
                description: ">=4 batch-some-expired events in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 4 },
            },
        },
        // Occasional market errors succeed on retry — alert on 3.
        {
            pattern: '"[B-SUB-002]"',
            metricName: "submitter-market-error-submission",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 submitter market errors in 1 min",
                metricConfig: { period: 60 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },
        // Tx timeout — may indicate misconfigured tx timeout config.
        // Logged multiple times per batch due to retries.
        {
            pattern: '"[B-SUB-003]"',
            metricName: "submitter-batch-submission-txn-timeout",
            alarm: {
                severity: Severity.SEV2,
                description: ">=4 batch submission tx timeouts in 2 hours",
                metricConfig: { period: 7200 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 4 },
            },
        },
        {
            pattern: '"[B-SUB-004]"',
            metricName: "submitter-batch-submission-failure",
            alarm: {
                severity: Severity.SEV2,
                description: ">=2 batch submission failures in 1 hour",
                metricConfig: { period: 3600 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 2 },
            },
        },
        // Sepolia has more tx confirmation issues — higher threshold + multi-period.
        {
            pattern: '"[B-SUB-006]"',
            metricName: "submitter-txn-confirmation-error",
            alarm: {
                severity: Severity.SEV2,
                description: `>=${isSepolia ? 20 : 5} submitter tx confirmation errors in ${isSepolia ? "2 hours" : "1 hour"}`,
                metricConfig: { period: 3600 },
                alarmConfig: {
                    evaluationPeriods: isSepolia ? 2 : 1,
                    datapointsToAlarm: isSepolia ? 2 : 1,
                    threshold: isSepolia ? 20 : 5,
                },
            },
        },
        {
            pattern: '"[B-SUB-500]"',
            metricName: "submitter-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: "any unexpected submitter error in 1 min",
                metricConfig: { period: 60 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 1 },
            },
        },
        {
            pattern: '"[B-SUB-500]"',
            metricName: "submitter-unexpected-error",
            alarm: {
                severity: Severity.SEV2,
                description: ">=3 unexpected submitter errors in 5 min",
                metricConfig: { period: 300 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 3 },
            },
        },

        // ── Reaper alarms ────────────────────────────────────────────────
        {
            pattern: '"[B-REAP-100]"',
            metricName: "reaper-expired-orders-found",
            alarm: {
                severity: Severity.SEV2,
                description: "expired committed orders found by reaper",
                metricConfig: { period: 60 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 1 },
            },
        },

        // ── Utils alarms ─────────────────────────────────────────────────
        // Failed to cancel proof can cause cascading failures with capacity estimation.
        {
            pattern: '"[B-UTL-001]"',
            metricName: "failed-to-cancel-proof",
            alarm: {
                severity: Severity.SEV2,
                description: "failed to cancel proof",
                metricConfig: { period: 60 },
                alarmConfig: { evaluationPeriods: 1, datapointsToAlarm: 1, threshold: 1 },
            },
        },
    ];
}
