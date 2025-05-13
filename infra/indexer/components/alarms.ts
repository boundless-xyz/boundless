import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { getServiceNameV1, Severity } from "../../util";
import { clients, Target } from "./targets";

const SERVICE_NAME_BASE = 'indexer';
const stackName = pulumi.getStack();
const serviceName = getServiceNameV1(stackName, SERVICE_NAME_BASE);
const config = new pulumi.Config();
const namespace = config.get("cloudwatchNamespace") || "indexer-monitor";
const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');
const alarmActions = boundlessAlertsTopicArn
    ? [boundlessAlertsTopicArn]
    : [];

export const createMetricAlarm = (
    metricName: string,
    severity: Severity,
    target?: Target,
    description?: string,
    metricConfig?: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric>,
    alarmConfig?: Partial<aws.cloudwatch.MetricAlarmArgs>,
): void => {
    metricName = target ? `${metricName}_${target.address}` : metricName;
    const metricFullName = target ? `${metricName}_${target.name}` : metricName;
    new aws.cloudwatch.MetricAlarm(`${serviceName}-${metricFullName}-${severity}-alarm`, {
        name: `${serviceName}-${metricName}-${severity}`,
        metricQueries: [
            {
                id: "a",
                metric: {
                    namespace: namespace,
                    metricName: metricName,
                    period: 60,
                    stat: "Sum",
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
        alarmDescription: `${severity} ${metricFullName} ${description}`,
        actionsEnabled: true,
        alarmActions,
        ...alarmConfig
    });
}

export const createSuccessRateAlarm = (
    target: Target,
    severity: Severity,
    description?: string,
    metricConfig?: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric>,
    alarmConfig?: Partial<aws.cloudwatch.MetricAlarmArgs>,
): void => {
    const metricName = `success_rate_for_${target.address}_${target.name}`;
    new aws.cloudwatch.MetricAlarm(`${serviceName}-${metricName}-${severity}-alarm`, {
        name: `${serviceName}-${metricName}-${severity}`,
        metricQueries: [
            {
                id: "a",
                metric: {
                    namespace: namespace,
                    metricName: `requests_number_from_${target.address}`,
                    period: 60,
                    stat: "Sum",
                    ...metricConfig
                },
                returnData: false,
            },
            {
                id: "b",
                metric: {
                    namespace: namespace,
                    metricName: `expired_requests_from_${target.address}`,
                    period: 60,
                    stat: "Sum",
                    ...metricConfig
                },
                returnData: false,
            },
            {
                id: "sr",
                expression: "IF(a > 0, (a - b) / a, 1)",
                label: "SuccessRate",
                returnData: true,
            },
        ],
        threshold: 1,
        comparisonOperator: 'LessThanThreshold',
        evaluationPeriods: 1,
        datapointsToAlarm: 1,
        treatMissingData: 'notBreaching',
        alarmDescription: `${severity} ${metricName} ${description}`,
        actionsEnabled: true,
        alarmActions,
        ...alarmConfig
    });
}
