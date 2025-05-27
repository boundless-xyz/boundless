import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import { config } from 'process';
import { getEnvVar, ChainId, getServiceNameV1, Severity } from "../../util";
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as fs from 'fs';

export const createProverAlarms = (
  serviceName: string,
  logGroup: pulumi.Output<aws.cloudwatch.LogGroup>,
  dependsOn: (pulumi.Resource | pulumi.Input<pulumi.Resource>)[],
  alarmActions: string[],
): void => {
  const createLogMetricFilter = (
    pattern: string,
    metricName: string,
    severity?: Severity,
  ): void => {
    // Generate a metric by filtering for the error code
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-${metricName}-${severity}-filter`, {
      name: `${serviceName}-${metricName}-${severity}-filter`,
      logGroupName: logGroup.name,
      metricTransformation: {
        namespace: `Boundless/Services/${serviceName}`,
        name: `${serviceName}-${metricName}-${severity}`,
        value: '1',
        defaultValue: '0',
      },
      pattern,
    }, { dependsOn });
  };

  const createErrorCodeAlarm = (
    pattern: string,
    metricName: string,
    severity: Severity,
    alarmConfig?: Partial<aws.cloudwatch.MetricAlarmArgs>,
    metricConfig?: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric>,
    description?: string
  ): void => {
    // Generate a metric by filtering for the error code
    createLogMetricFilter(pattern, metricName, severity);

    // Create an alarm for the metric
    new aws.cloudwatch.MetricAlarm(`${serviceName}-${metricName}-${severity}-alarm`, {
      name: `${serviceName}-${metricName}-${severity}`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: `Boundless/Services/${serviceName}`,
            metricName: `${serviceName}-${metricName}-${severity}`,
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
      alarmDescription: `${severity} ${metricName} ${description ?? ''}`,
      actionsEnabled: true,
      alarmActions,
      ...alarmConfig
    });
  }

  // Alarms across the entire prover.
  // Note: AWS has a limit of 5 filter patterns containing regex for each log group
  // https://docs.aws.amazon.com/AmazonCloudWatch/latest/logs/FilterAndPattern.html

  // [Regex] 3 unexpected errors across the entire prover in 5 minutes triggers a SEV2 alarm
  createErrorCodeAlarm('%\[B-[A-Z]+-500\]%', 'unexpected-errors', Severity.SEV2, {
    threshold: 5,
  }, { period: 300 });

  // [Regex] 10 errors of any kind across the entire prover within an hour triggers a SEV2 alarm
  createErrorCodeAlarm('%\[B-[A-Z]+-\d+\]%', 'assorted-errors', Severity.SEV2, {
    threshold: 10,
  }, { period: 3600 });

  // Matches on any ERROR log that does NOT contain an error code. Ensures we don't miss any errors.
  // Don't match on INTERNAL_ERROR which is sometimes returned by our dependencies e.g. Bonsai on retryable errors.
  createErrorCodeAlarm('ERROR -"[B-" -"INTERNAL_ERROR"', 'error-without-code', Severity.SEV2);

  // Alarms for low balances
  createErrorCodeAlarm('WARN "[B-BAL-ETH]"', 'low-balance-alert-eth', Severity.SEV2);
  createErrorCodeAlarm('WARN "[B-BAL-STK]"', 'low-balance-alert-stk', Severity.SEV2);
  createErrorCodeAlarm('ERROR "[B-BAL-ETH]"', 'low-balance-alert-eth', Severity.SEV1);
  createErrorCodeAlarm('ERROR "[B-BAL-STK]"', 'low-balance-alert-stk', Severity.SEV1);

  // Alarms at the supervisor level
  //
  // 5 supervisor restarts within 15 mins triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-SUP-RECOVER]"', 'supervisor-recover-errors', Severity.SEV2, {
    threshold: 5,
  }, { period: 900 });

  // 1 supervisor fault triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-SUP-FAULT]"', 'supervisor-fault-errors', Severity.SEV2, {
    threshold: 1,
  });

  //
  // Alarms for specific services and error codes.
  // Matching without using regex to avoid the AWS limit.
  //

  //
  // DB
  //
  // 1 db locked error triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-DB-001]"', 'db-locked-error', Severity.SEV2);

  // 1 db pool timeout error triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-DB-002]"', 'db-pool-timeout-error', Severity.SEV2);

  // 1 db unexpected error triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-DB-500]"', 'db-unexpected-error', Severity.SEV2);

  //
  // Storage
  //
  // 3 http errors (e.g. rate limiting, etc.) within 5 minutes triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-STR-002]"', 'storage-http-error', Severity.SEV2, {
    threshold: 3,
  }, { period: 300 });

  // 1 unexpected storage error triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-STR-500]"', 'storage-unexpected-error', Severity.SEV2);

  //
  // Market Monitor
  //
  // 3 event polling errors within 5 minutes in the market monitor triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-MM-501]"', 'market-monitor-event-polling-error', Severity.SEV2, {
    threshold: 3,
  }, { period: 300 });

  // 10 event polling errors within 30 minutes in the market monitor triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-MM-501]"', 'market-monitor-event-polling-error', Severity.SEV1, {
    threshold: 10,
  }, { period: 1800 });

  // 5 log processing errors within 15 minutes in the market monitor triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-MM-502]"', 'market-monitor-log-processing-error', Severity.SEV2, {
    threshold: 5,
  }, { period: 900 });

  // Any 1 unexpected error in the market monitor triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-MM-500]"', 'market-monitor-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the market monitor triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-MM-500]"', 'market-monitor-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });

  //
  // Chain Monitor
  //

  // RPC errors can occur transiently. 
  // If we see 5 rpc errors within 1 hour in the chain monitor trigger a SEV2 alarm to investigate.
  createErrorCodeAlarm('"[B-CHM-400]"', 'chain-monitor-rpc-error', Severity.SEV2, {
    threshold: 5,
  }, { period: 3600 });

  // Any 1 unexpected error in the on-chain market monitor triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-CHM-500]"', 'chain-monitor-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the chain monitor triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-CHM-500]"', 'chain-monitor-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });

  //
  // Off-chain Market Monitor
  //

  // 10 websocket errors within 1 hour in the off-chain market monitor triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-OMM-001]"', 'off-chain-market-monitor-websocket-error', Severity.SEV1, {
    threshold: 10,
  }, { period: 3600 });

  // 3 websocket errors within 15 minutes in the off-chain market monitor triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-OMM-001]"', 'off-chain-market-monitor-websocket-error', Severity.SEV2, {
    threshold: 3,
  }, { period: 900 });

  // Any 1 unexpected error in the off-chain market monitor triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-OMM-500]"', 'off-chain-market-monitor-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the off-chain market monitor triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-OMM-500]"', 'off-chain-market-monitor-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });

  //
  // Order Picker
  //
  // Any 1 unexpected error in the order picker triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-OP-500]"', 'order-picker-unexpected-error', Severity.SEV2, {
    threshold: 1,
  });

  // Create a metric for errors when fetching images/inputs but don't alarm as could be user error.
  // Note: This is a pattern to match "[B-OP-001]" OR "[B-OP-002]"
  createLogMetricFilter('?"[B-OP-001]" ?"[B-OP-002]"', 'order-picker-fetch-error');
  createErrorCodeAlarm('?"[B-OP-001]" ?"[B-OP-002]"', 'order-picker-fetch-error', Severity.SEV2, {
    threshold: 3,
  }, { period: 900 });

  // 3 unexpected errors within 5 minutes in the order picker triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-OP-500]"', 'order-picker-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });

  //
  // Order Monitor
  //
  // Any 1 unexpected error in the order monitor triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-OM-500]"', 'order-monitor-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the order monitor triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-OM-500]"', 'order-monitor-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });

  // Create metrics for scenarios where we fail to lock an order that we wanted to lock.
  // Don't alarm as this is expected behavior when another prover locked before us.
  // If we fail to lock an order because the tx fails for some reason.
  createLogMetricFilter('"[B-OM-007]"', 'order-monitor-lock-tx-failed');
  // If we fail to lock an order because we saw an event indicating another prover locked before us.
  createLogMetricFilter('"[B-OM-009]"', 'order-monitor-already-locked');

  // If we fail to lock an order because we don't have enough stake balance, SEV2.
  createErrorCodeAlarm('"[B-OM-010]"', 'order-monitor-insufficient-balance', Severity.SEV2);

  // 3 lock tx not confirmed errors within 1 hour in the order monitor triggers a SEV2 alarm.
  // This may indicate a misconfiguration of the tx timeout config.
  createErrorCodeAlarm('"[B-OM-006]"', 'order-monitor-lock-tx-not-confirmed', Severity.SEV2, {
    threshold: 3,
  }, { period: 3600 });



  //
  // Prover
  //
  // Any 1 unexpected error in the prover triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-PRO-500]"', 'prover-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the prover triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-PRO-500]"', 'prover-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });

  // 2 proving failed errors within 30 minutes in the prover triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-PRO-501]"', 'prover-proving-failed', Severity.SEV2, {
    threshold: 2,
  }, { period: 1800 }, "Proving with retries failed 2 times within 30 minutes");

  // Aggregator
  // Any 1 unexpected error in the aggregator triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-AGG-500]"', 'aggregator-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the aggregator triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-AGG-500]"', 'aggregator-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });

  //
  // Submitter
  //
  // Any 1 request expired before submission triggers a SEV2 alarm.
  // Typically this is due to proving/aggregating/submitting taking longer than expected.
  createErrorCodeAlarm('"[B-SUB-001]"', 'submitter-request-expired-before-submission', Severity.SEV2);

  // Any 1 request expired before submission triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-SUB-002]"', 'submitter-market-error-submission', Severity.SEV2);

  // 2 failures to submit a batch within 1 hour in the submitter triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-SUB-003]"', 'submitter-batch-submission-failure', Severity.SEV2, {
    threshold: 2,
  }, { period: 3600 });

  // 3 txn confirmation errors within 1 hour in the submitter triggers a SEV2 alarm. 
  // This may indicate a misconfiguration of the tx timeout config.
  createErrorCodeAlarm('"[B-SUB-004]"', 'submitter-txn-confirmation-error', Severity.SEV2, {
    threshold: 3,
  }, { period: 3600 });

  // Any 1 unexpected error in the submitter triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-SUB-500]"', 'submitter-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the submitter triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-SUB-500]"', 'submitter-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });
}
