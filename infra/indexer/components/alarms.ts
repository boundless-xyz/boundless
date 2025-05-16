import * as aws from "@pulumi/aws";
import { Severity } from "../../util";
import { Target } from "./targets";

export const buildCreateMetricFns = (serviceName: string, namespace: string, alarmActions: string[]) => {
    const createMetricAlarm = (
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

    const createSuccessRateAlarm = (
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
                        metricName: `fulfilled_requests_number_from_${target.address}`,
                        period: 3600,
                        stat: "Sum",
                        ...metricConfig
                    },
                    returnData: false,
                },
                {
                    id: "b",
                    metric: {
                        namespace: namespace,
                        metricName: `expired_requests_number_from_${target.address}`,
                        period: 3600,
                        stat: "Sum",
                        ...metricConfig
                    },
                    returnData: false,
                },
                {
                    id: "sr",
                    expression: "IF(a + b > 0, a / (a + b), 1)",
                    label: "SuccessRate",
                    returnData: true,
                },
            ],
            threshold: 1,
            comparisonOperator: 'LessThanThreshold',
            evaluationPeriods: 12,
            datapointsToAlarm: 1,
            treatMissingData: 'notBreaching',
            alarmDescription: `${severity} ${metricName} ${description}`,
            actionsEnabled: true,
            alarmActions,
            ...alarmConfig
        });
    }

    return {
        createMetricAlarm,
        createSuccessRateAlarm,
    }
}

