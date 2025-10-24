import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import {ChainId, Severity} from "../../util";
import {BaseComponent, BaseComponentConfig} from "./BaseComponent";

export interface MetricAlarmConfig extends BaseComponentConfig {
    serviceName: string,
    logGroupName: string,
    alertsTopicArns: string[];
}

// Creates and manages general metric filters and alarms that can be common to multiple components
export class MetricAlarmComponent extends BaseComponent {
    public readonly logGroup: pulumi.Output<aws.cloudwatch.LogGroup>

    constructor(config: MetricAlarmConfig) {
        super(config, "boundless-bento");

        // Try to get an existing log group
        const existingLogGroup = pulumi.output(aws.cloudwatch.getLogGroup({
            name: config.logGroupName,
        }).catch(() => undefined));

        this.logGroup = existingLogGroup.apply(existing => {
            if (existing) {
                // Import the existing log group into a LogGroup resource
                return new aws.cloudwatch.LogGroup(`${config.serviceName}-log-group`, {
                    name: existing.name,
                    retentionInDays: existing.retentionInDays,
                }, {import: existing.id});
            }

            // Otherwise create a new log group
            return new aws.cloudwatch.LogGroup(`${config.serviceName}-log-group`, {
                name: config.logGroupName,
                retentionInDays: 0,
            });
        });

        this.createMetricAlarms(config)
    }

    protected createLogMetricFilter = (
        config: MetricAlarmConfig,
        pattern: string,
        metricName: string,
        severity?: Severity,
    ): aws.cloudwatch.LogMetricFilter => {
        // Generate a metric by filtering for the error code
        return new aws.cloudwatch.LogMetricFilter(`${config.serviceName}-${metricName}-${severity}-${config.stackName}-filter`, {
            name: `${config.serviceName}-${metricName}-${severity}-${config.stackName}-filter`,
            logGroupName: config.logGroupName,
            metricTransformation: {
                namespace: `Boundless/Services/${config.serviceName}/${config.stackName}`,
                name: `${config.serviceName}-${metricName}-${severity}-${config.stackName}`,
                value: '1',
                defaultValue: '0',
            },
            pattern: pattern,
        }, {dependsOn: this.logGroup});
    };

    protected createErrorCodeAlarm = (
        config: MetricAlarmConfig,
        pattern: string,
        metricName: string,
        severity: Severity,
        alarmConfig?: Partial<aws.cloudwatch.MetricAlarmArgs>,
        metricConfig?: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric>,
        description?: string
    ): aws.cloudwatch.MetricAlarm => {
        // Generate a metric by filtering for the error code
        this.createLogMetricFilter(config, pattern, metricName, severity);

        // Create an alarm for the metric
        return new aws.cloudwatch.MetricAlarm(`${config.serviceName}-${metricName}-${severity}-${config.stackName}-alarm`, {
            name: `${config.serviceName}-${metricName}-${severity}-${config.stackName}`,
            metricQueries: [
                {
                    id: 'm1',
                    metric: {
                        namespace: `Boundless/Services/${config.serviceName}/${config.stackName}`,
                        metricName: `${config.serviceName}-${metricName}-${severity}-${config.stackName}`,
                        period: 60,
                        stat: 'Sum',
                        ...metricConfig
                    },
                    returnData: true,
                },
            ],
            threshold: 1,
            comparisonOperator: 'GreaterThanOrEqualToThreshold',
            evaluationPeriods: 1,
            datapointsToAlarm: 1,
            treatMissingData: 'notBreaching',
            alarmDescription: `${severity} ${metricName} ${config.stackName} ${description ?? ''}`,
            actionsEnabled: true,
            alarmActions: config.alertsTopicArns ?? [],
            tags: {
                Name: `${config.serviceName}-${metricName}-${severity}-${config.stackName}`,
                Environment: this.config.environment,
                Project: "boundless-bento-cluster",
            },
            ...alarmConfig
        });
    };

    private createMetricAlarms = (config: MetricAlarmConfig): void => {
        // Unexpected error threshold for entire broker.
        const brokerUnexpectedErrorThreshold = 5;

        // Alarms across the entire prover.
        // Note: AWS has a limit of 5 filter patterns containing regex for each log group
        // https://docs.aws.amazon.com/AmazonCloudWatch/latest/logs/FilterAndPatternSyntax.html

        // [Regex] 5 unexpected errors across the entire prover in 5 minutes triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '%\[B-[A-Z]+-500\]%', 'unexpected-errors', Severity.SEV2, {
            threshold: brokerUnexpectedErrorThreshold,
        }, {period: 300});

        // [Regex] 10 errors of any kind across the entire prover within an hour triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '%\[B-[A-Z]+-\d+\]%', 'assorted-errors', Severity.SEV2, {
            threshold: 10,
        }, {period: 3600});

        // Matches on any ERROR log that does NOT contain an error code. Ensures we don't miss any errors.
        // Don't match on INTERNAL_ERROR which is sometimes returned by our dependencies e.g. Bonsai on retryable errors.
        this.createErrorCodeAlarm(config, 'ERROR -"[B-" -"INTERNAL_ERROR"', 'error-without-code', Severity.SEV2);

    };
}

export interface ManagerMetricAlarmConfig extends MetricAlarmConfig {
    chainId: string
}

// Creates and manages metric filters and alarms that are specific to the manager component
export class ManagerMetricAlarmComponent extends MetricAlarmComponent {

    constructor(config: ManagerMetricAlarmConfig) {
        super(config);
        this.createManagerMetricAlarms(config)
    }

    private createManagerMetricAlarms = (config: ManagerMetricAlarmConfig): void => {
        // Unexpected error threshold for entire broker.
        const supervisorUnexpectedErrorThreshold = 5;
        // Unexpected error threshold for individual services.
        const serviceUnexpectedErrorThreshold = 2;
        const serviceUnexpectedErrorThresholdSev1 = 3;

        // Alarms for low balances. Once breached, the log continues on every tx, so we use a 6 hour period
        // to prevent noise from the alarm being triggered multiple times.
        this.createErrorCodeAlarm(config, 'WARN "[B-BAL-ETH]"', 'low-balance-alert-eth', Severity.SEV2, {
            threshold: 1,
        }, {period: 3600});
        this.createErrorCodeAlarm(config, 'WARN "[B-BAL-STK]"', 'low-balance-alert-stk', Severity.SEV2, {
            threshold: 1,
        }, {period: 3600});
        this.createErrorCodeAlarm(config, 'ERROR "[B-BAL-ETH]"', 'low-balance-alert-eth', Severity.SEV1, {
            threshold: 1,
        }, {period: 3600});
        this.createErrorCodeAlarm(config, 'ERROR "[B-BAL-STK]"', 'low-balance-alert-stk', Severity.SEV1, {
            threshold: 1,
        }, {period: 3600});

        // Alarms at the supervisor level
        //
        // 5 supervisor restarts within 15 mins triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '"[B-SUP-RECOVER]"', 'supervisor-recover-errors', Severity.SEV2, {
            threshold: supervisorUnexpectedErrorThreshold,
        }, {period: 900});

        // 2 supervisor fault within 30 minutes triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '"[B-SUP-FAULT]"', 'supervisor-fault-errors', Severity.SEV2, {
            threshold: 2,
        }, {period: 1800});

        //
        // Alarms for specific services and error codes.
        // Matching without using regex to avoid the AWS limit.
        //

        //
        // DB
        //
        const dbLockedErrorThreshold = 1;
        // 2 db locked error within 30 minutes triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '"[B-DB-001]"', 'db-locked-error', Severity.SEV2, {
            threshold: dbLockedErrorThreshold,
        }, {period: 1800}, "DB locked error 2 times within 30 minutes");

        // 2 db pool timeout error within 30 minutes triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '"[B-DB-002]"', 'db-pool-timeout-error', Severity.SEV2, {
            threshold: dbLockedErrorThreshold,
        }, {period: 1800}, "DB pool timeout error 2 times within 30 minutes");

        // 2 db unexpected error within 30 minutes triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '"[B-DB-500]"', 'db-unexpected-error', Severity.SEV2, {
            threshold: dbLockedErrorThreshold,
        }, {period: 1800}, "DB unexpected error 2 times within 30 minutes");

        //
        // Storage
        //
        // 3 http errors (e.g. rate limiting, etc.) within 5 minutes triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '"[B-STR-002]"', 'storage-http-error', Severity.SEV2, {
            threshold: 3,
        }, {period: 300});

        // 2 unexpected storage errors triggers a SEV2 alarm
        this.createErrorCodeAlarm(config, '"[B-STR-500]"', 'storage-unexpected-error', Severity.SEV2, {threshold: 2});

        //
        // Market Monitor
        //
        // 3 event polling errors within 5 minutes in the market monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-MM-501]"', 'market-monitor-event-polling-error', Severity.SEV2, {
            threshold: 3,
        }, {period: 300});

        // 10 event polling errors within 30 minutes in the market monitor triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-MM-501]"', 'market-monitor-event-polling-error', Severity.SEV1, {
            threshold: 10,
        }, {period: 1800});

        // 5 log processing errors within 15 minutes in the market monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-MM-502]"', 'market-monitor-log-processing-error', Severity.SEV2, {
            threshold: 5,
        }, {period: 900});

        // Any 2 unexpected errors within 30 minutes in the market monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-MM-500]"', 'market-monitor-unexpected-error', Severity.SEV2, {threshold: 2}, {period: 1800});

        // 3 unexpected errors within 5 minutes in the market monitor triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-MM-500]"', 'market-monitor-unexpected-error', Severity.SEV1, {
            threshold: 3,
        }, {period: 300});

        //
        // Chain Monitor
        //

        // RPC errors can occur transiently.
        // If we see 5 rpc errors within 1 hour in the chain monitor trigger a SEV2 alarm to investigate.
        this.createErrorCodeAlarm(config, '"[B-CHM-400]"', 'chain-monitor-rpc-error', Severity.SEV2, {
            threshold: 5,
        }, {period: 3600});

        // Any 2 unexpected errors within 30 minutes in the chain monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-CHM-500]"', 'chain-monitor-unexpected-error', Severity.SEV2, {
            threshold: 2,
        }, {period: 1800});

        // 3 unexpected errors within 5 minutes in the chain monitor triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-CHM-500]"', 'chain-monitor-unexpected-error', Severity.SEV1, {
            threshold: 3,
        }, {period: 300});

        //
        // Off-chain Market Monitor
        //

        // 10 websocket errors within 1 hour in the off-chain market monitor triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-OMM-001]"', 'off-chain-market-monitor-websocket-error', Severity.SEV1, {
            threshold: 10,
        }, {period: 3600});

        // 3 websocket errors within 15 minutes in the off-chain market monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-OMM-001]"', 'off-chain-market-monitor-websocket-error', Severity.SEV2, {
            threshold: 3,
        }, {period: 900});

        // Any 2 unexpected errors within 30 minutes in the off-chain market monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-OMM-500]"', 'off-chain-market-monitor-unexpected-error', Severity.SEV2, {
            threshold: 2,
        }, {period: 1800});

        // 3 unexpected errors within 5 minutes in the off-chain market monitor triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-OMM-500]"', 'off-chain-market-monitor-unexpected-error', Severity.SEV1, {
            threshold: 3,
        }, {period: 300});

        //
        // Order Picker
        //
        // Any 2 unexpected errors within 30 minutes in the order picker triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-OP-500]"', 'order-picker-unexpected-error', Severity.SEV2, {
            threshold: 2,
        }, {period: 1800});

        // Create a metric for errors when fetching images/inputs but don't alarm as could be user error.
        // Note: This is a pattern to match "[B-OP-001]" OR "[B-OP-002]"
        this.createLogMetricFilter(config, '?"[B-OP-001]" ?"[B-OP-002]"', 'order-picker-fetch-error');

        // 3 unexpected errors within 5 minutes in the order picker triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-OP-500]"', 'order-picker-unexpected-error', Severity.SEV1, {
            threshold: 3,
        }, {period: 300});

        // 3 rpc errors within 15 minutes in the order picker triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-OP-005]"', 'order-picker-rpc-error', Severity.SEV2, {
            threshold: 3,
        }, {period: 900});

        //
        // Order Monitor
        //
        // Any 2 unexpected errors within 30 minutes in the order monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-OM-500]"', 'order-monitor-unexpected-error', Severity.SEV2, {
            threshold: 2,
        }, {period: 1800});

        // 4 unexpected errors within 5 minutes in the order monitor triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-OM-500]"', 'order-monitor-unexpected-error', Severity.SEV1, {
            threshold: 4,
        }, {period: 300});

        // Create metrics for scenarios where we fail to lock an order that we wanted to lock.
        // Don't alarm as this is expected behavior when another prover locked before us.
        // If we fail to lock an order because the tx fails for some reason.
        this.createLogMetricFilter(config, '"[B-OM-007]"', 'order-monitor-lock-tx-failed');
        // If we fail to lock an order because we saw an event indicating another prover locked before us.
        this.createLogMetricFilter(config, '"[B-OM-009]"', 'order-monitor-already-locked');

        // If we fail to lock an order twice within 2 hours because we don't have enough stake balance, SEV2.
        this.createErrorCodeAlarm(config, '"[B-OM-010]"', 'order-monitor-insufficient-balance', Severity.SEV2, {
            threshold: 2,
        }, {period: 7200});

        // For Sepolia, we see more tx not confirmed errors than other chains, so we use a higher threshold.
        // Other networks, 3 lock tx not confirmed errors within 1 hour in the order monitor triggers a SEV2 alarm.
        // This may indicate a misconfiguration of the tx timeout config.
        this.createErrorCodeAlarm(config, '"[B-OM-006]"', 'order-monitor-lock-tx-not-confirmed', Severity.SEV2, {
            threshold: config.chainId == ChainId.ETH_SEPOLIA ? 10 : 3,
        }, {period: 3600});

        // 3 rpc errors within 15 minutes in the order monitor triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-OM-011]"', 'order-monitor-rpc-error', Severity.SEV2, {
            threshold: 3,
        }, {period: 900});

        //
        // Prover
        //
        // Any 2 unexpected errors within 30 minutes in the prover triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-PRO-500]"', 'prover-unexpected-error', Severity.SEV2, {
            threshold: serviceUnexpectedErrorThreshold,
        }, {period: 1800});

        // 3 unexpected errors within 5 minutes in the prover triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-PRO-500]"', 'prover-unexpected-error', Severity.SEV1, {
            threshold: serviceUnexpectedErrorThresholdSev1,
        }, {period: 300});

        // 2 proving failed errors within 30 minutes in the prover triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-PRO-501]"', 'prover-proving-failed', Severity.SEV2, {
            threshold: 2,
        }, {period: 1800}, "Proving with retries failed 2 times within 30 minutes");

        // Aggregator
        //
        // 2 batch failure to compress within 2 hours triggers a SEV2 alarm. This indicates a fault with the prover.
        this.createErrorCodeAlarm(config, '"[B-AGG-400]"', 'aggregator-compression-error', Severity.SEV2, {
            threshold: 2,
        }, {period: 7200});

        // Any 2 unexpected errors within 30 minutes in the aggregator triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-AGG-500]"', 'aggregator-unexpected-error', Severity.SEV2, {
            threshold: serviceUnexpectedErrorThreshold,
        }, {period: 1800});

        // 3 unexpected errors within 5 minutes in the aggregator triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-AGG-500]"', 'aggregator-unexpected-error', Severity.SEV1, {
            threshold: serviceUnexpectedErrorThresholdSev1,
        }, {period: 300});

        // An edge case to expire in the aggregator, also indicates that a slashed order.
        this.createErrorCodeAlarm(config, '"[B-AGG-600]"', 'aggregator-order-expired', Severity.SEV2, {
            threshold: 2,
        }, {period: 3600});

        //
        // Proving engine
        //

        // Track internal errors as a metric, but these errors are expected to happen occasionally.
        // and are retried and covered by other alarms.
        this.createLogMetricFilter(config, '"[B-BON-008]"', 'proving-engine-internal-error');

        //
        // Submitter
        //
        // Two cases in an hour where all requests in a batch expired before submission triggers a SEV2 alarm.
        // Typically this is due to proving/aggregating/submitting taking longer than expected.
        // Note this gets logged multiple times per batch submission due to retries, so we use a higher threshold.
        this.createErrorCodeAlarm(config, '"[B-SUB-001]"', 'submitter-requests-expired-before-submission', Severity.SEV2, {
            threshold: 4,
        }, {period: 3600}, "All requests in a batch expired before submission twice in an hour");

        // Two cases where some requests in a batch expired before submission triggers a SEV2 alarm.
        // Typically this is due to proving/aggregating/submitting taking longer than expected.
        // Note this gets logged multiple times per batch submission due to retries, so we use a higher threshold.
        this.createErrorCodeAlarm(config, '"[B-SUB-005]"', 'submitter-some-requests-expired-before-submission', Severity.SEV2, {
            threshold: 4,
        }, {period: 3600}, "Some requests in a batch expired before submission twice in an hour");

        // Any 3 market errors triggers a SEV2 alarm. Note we occasionally see these errors and on retry they
        // succeed, so we alert on 3
        this.createErrorCodeAlarm(config, '"[B-SUB-002]"', 'submitter-market-error-submission', Severity.SEV2, {
            threshold: 3,
        });

        // 4 failures to submit a batch within 2 hours due to timeouts in the submitter triggers a SEV2 alarm.
        // This may indicate a misconfiguration of the tx timeout config.
        // Note this gets logged multiple times per batch submission due to retries, so we use a higher threshold.
        this.createErrorCodeAlarm(config, '"[B-SUB-003]"', 'submitter-batch-submission-txn-timeout', Severity.SEV2, {
            threshold: 4,
        }, {period: 7200});

        // 2 failures to submit a batch within 1 hour for any reason in the submitter triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-SUB-004]"', 'submitter-batch-submission-failure', Severity.SEV2, {
            threshold: 2,
        }, {period: 3600});

        // 5 (10 on Sepolia since tx confirmation issues happen more frequently) individual txn confirmation
        // errors within 1 hour in the submitter triggers a SEV2 alarm.
        // Note, we retry on individual txn confirmation errors, so this does not necessarily indicate
        // the batch was not submitted.
        // This may indicate a misconfiguration of the tx timeout config.
        this.createErrorCodeAlarm(config, '"[B-SUB-006]"', 'submitter-txn-confirmation-error', Severity.SEV2, {
            threshold: config.chainId == ChainId.ETH_SEPOLIA ? 20 : 5,
            evaluationPeriods: config.chainId == ChainId.ETH_SEPOLIA ? 2 : 1,
            datapointsToAlarm: config.chainId == ChainId.ETH_SEPOLIA ? 2 : 1,
        }, {period: 3600});

        // Any 1 unexpected error in the submitter triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-SUB-500]"', 'submitter-unexpected-error', Severity.SEV2);

        // 3 unexpected errors within 5 minutes in the submitter triggers a SEV1 alarm.
        this.createErrorCodeAlarm(config, '"[B-SUB-500]"', 'submitter-unexpected-error', Severity.SEV1, {
            threshold: 3,
        }, {period: 300});

        //
        // Reaper
        //

        // Any expired committed orders by the broker found triggers a SEV2 alarm.
        this.createErrorCodeAlarm(config, '"[B-REAP-100]"', 'reaper-expired-orders-found', Severity.SEV2);

        //
        // Utils
        //
        // Any 1 failed to cancel proof triggers a SEV2 alarm. This can cause cascading failures with
        // capacity estimation.
        this.createErrorCodeAlarm(config, '"[B-UTL-001]"', 'failed-to-cancel-proof', Severity.SEV2);
    }
}
