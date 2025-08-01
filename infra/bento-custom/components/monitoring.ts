import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export async function setupMonitoring(
    name: string,
    instances: {
        execAgents: any,
        snarkAgent: any,
        gpuProvers: any,
        auxAgent: any,
        ec2Broker: any,
        bentoAPI: any
    },
    database: any,
    cache: any,
    cluster: any,
    tags: Record<string, string>
) {
    // Get current AWS region
    const region = await aws.getRegion();
    // Create SNS topic for alerts
    const alertsTopic = new aws.sns.Topic(`${name}-alerts`, {
        name: `${name}-alerts`,
        tags: {
            ...tags,
            Name: `${name}-alerts`,
        },
    });

    // EC2 Instance Health Monitoring
    const createInstanceAlarms = (instanceId: pulumi.Output<string>, instanceName: string) => {
        // CPU Utilization
        new aws.cloudwatch.MetricAlarm(`${name}-${instanceName}-cpu-high`, {
            name: `${name}-${instanceName}-cpu-high`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "CPUUtilization",
            namespace: "AWS/EC2",
            period: 300,
            statistic: "Average",
            threshold: 80,
            alarmDescription: `High CPU utilization on ${instanceName}`,
            alarmActions: [alertsTopic.arn],
            dimensions: {
                InstanceId: instanceId,
            },
            tags: {
                ...tags,
                Name: `${name}-${instanceName}-cpu-high`,
            },
        });

        // Status Check Failed
        new aws.cloudwatch.MetricAlarm(`${name}-${instanceName}-status-check`, {
            name: `${name}-${instanceName}-status-check`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "StatusCheckFailed",
            namespace: "AWS/EC2",
            period: 300,
            statistic: "Maximum",
            threshold: 0,
            alarmDescription: `Status check failed on ${instanceName}`,
            alarmActions: [alertsTopic.arn],
            dimensions: {
                InstanceId: instanceId,
            },
            tags: {
                ...tags,
                Name: `${name}-${instanceName}-status-check`,
            },
        });

        // Memory utilization (requires CloudWatch agent)
        new aws.cloudwatch.MetricAlarm(`${name}-${instanceName}-memory-high`, {
            name: `${name}-${instanceName}-memory-high`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "mem_used_percent",
            namespace: "CWAgent",
            period: 300,
            statistic: "Average",
            threshold: 85,
            alarmDescription: `High memory utilization on ${instanceName}`,
            alarmActions: [alertsTopic.arn],
            dimensions: {
                InstanceId: instanceId,
            },
            tags: {
                ...tags,
                Name: `${name}-${instanceName}-memory-high`,
            },
        });

        // Disk utilization
        new aws.cloudwatch.MetricAlarm(`${name}-${instanceName}-disk-high`, {
            name: `${name}-${instanceName}-disk-high`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "disk_used_percent",
            namespace: "CWAgent",
            period: 300,
            statistic: "Average",
            threshold: 80,
            alarmDescription: `High disk utilization on ${instanceName}`,
            alarmActions: [alertsTopic.arn],
            dimensions: {
                InstanceId: instanceId,
                device: "/dev/nvme0n1p1",
                fstype: "ext4",
                path: "/",
            },
            tags: {
                ...tags,
                Name: `${name}-${instanceName}-disk-high`,
            },
        });
    };

    // Create alarms for each instance type
    // TODO: Update monitoring for ECS services instead of EC2 instances
    // createInstanceAlarms(instances.execAgents.instance.id, "exec-agents");
    // createInstanceAlarms(instances.snarkAgent.instance.id, "snark-agent");

    // GPU Prover instances
    // TODO: Update monitoring for ECS services instead of EC2 instances
    // instances.gpuProvers.instances.forEach((instance: any, index: number) => {
    //     createInstanceAlarms(instance.id, `gpu-prover-${index}`);
    //     
    //     // GPU-specific monitoring
    //     new aws.cloudwatch.MetricAlarm(`${name}-gpu-prover-${index}-gpu-util`, {
    //         name: `${name}-gpu-prover-${index}-gpu-util`,
    //         comparisonOperator: "LessThanThreshold",
    //         evaluationPeriods: 3,
    //         metricName: "DCGM_FI_DEV_GPU_UTIL",
    //         namespace: "CWAgent",
    //         period: 300,
    //         statistic: "Average",
    //         threshold: 5,
    //         alarmDescription: `GPU ${index} utilization too low - possible issue`,
    //         alarmActions: [alertsTopic.arn],
    //         dimensions: {
    //             InstanceId: instance.id,
    //             gpu: "0",
    //         },
    //         tags: {
    //             ...tags,
    //             Name: `${name}-gpu-prover-${index}-gpu-util`,
    //         },
    //     });
    // });

    // EC2 Broker monitoring
    // Note: EC2 broker instances are managed by Auto Scaling Group
    // We'll monitor the ASG health and create alarms for typical EC2 metrics
    new aws.cloudwatch.MetricAlarm(`${name}-broker-cpu-high`, {
        name: `${name}-broker-cpu-high`,
        comparisonOperator: "GreaterThanThreshold",
        evaluationPeriods: 2,
        metricName: "CPUUtilization",
        namespace: "AWS/EC2",
        period: 300,
        statistic: "Average",
        threshold: 80,
        alarmDescription: "High CPU utilization on broker instance",
        alarmActions: [alertsTopic.arn],
        dimensions: {
            AutoScalingGroupName: instances.ec2Broker.instance.name,
        },
        tags: {
            ...tags,
            Name: `${name}-broker-cpu-high`,
        },
    });

    new aws.cloudwatch.MetricAlarm(`${name}-broker-memory-high`, {
        name: `${name}-broker-memory-high`,
        comparisonOperator: "GreaterThanThreshold",
        evaluationPeriods: 2,
        metricName: "mem_used_percent",
        namespace: "Boundless/Broker",
        period: 300,
        statistic: "Average",
        threshold: 85,
        alarmDescription: "High memory utilization on broker instance",
        alarmActions: [alertsTopic.arn],
        dimensions: {
            AutoScalingGroupName: instances.ec2Broker.instance.name,
        },
        tags: {
            ...tags,
            Name: `${name}-broker-memory-high`,
        },
    });

    new aws.cloudwatch.MetricAlarm(`${name}-broker-disk-high`, {
        name: `${name}-broker-disk-high`,
        comparisonOperator: "GreaterThanThreshold",
        evaluationPeriods: 2,
        metricName: "disk_used_percent",
        namespace: "Boundless/Broker",
        period: 300,
        statistic: "Average",
        threshold: 80,
        alarmDescription: "High disk utilization on broker instance",
        alarmActions: [alertsTopic.arn],
        dimensions: {
            AutoScalingGroupName: instances.ec2Broker.instance.name,
        },
        tags: {
            ...tags,
            Name: `${name}-broker-disk-high`,
        },
    });

    // RDS Database Monitoring
    const createDatabaseAlarms = () => {
        // Database CPU
        new aws.cloudwatch.MetricAlarm(`${name}-db-cpu-high`, {
            name: `${name}-db-cpu-high`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "CPUUtilization",
            namespace: "AWS/RDS",
            period: 300,
            statistic: "Average",
            threshold: 75,
            alarmDescription: "High CPU utilization on RDS database",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                DBInstanceIdentifier: database.instance.id,
            },
            tags: {
                ...tags,
                Name: `${name}-db-cpu-high`,
            },
        });

        // Database connections
        new aws.cloudwatch.MetricAlarm(`${name}-db-connections-high`, {
            name: `${name}-db-connections-high`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "DatabaseConnections",
            namespace: "AWS/RDS",
            period: 300,
            statistic: "Average",
            threshold: 50, // Adjust based on instance class
            alarmDescription: "High number of database connections",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                DBInstanceIdentifier: database.instance.id,
            },
            tags: {
                ...tags,
                Name: `${name}-db-connections-high`,
            },
        });

        // Database storage space
        new aws.cloudwatch.MetricAlarm(`${name}-db-storage-low`, {
            name: `${name}-db-storage-low`,
            comparisonOperator: "LessThanThreshold",
            evaluationPeriods: 1,
            metricName: "FreeStorageSpace",
            namespace: "AWS/RDS",
            period: 300,
            statistic: "Average",
            threshold: 2000000000, // 2GB in bytes
            alarmDescription: "Low free storage space on RDS database",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                DBInstanceIdentifier: database.instance.id,
            },
            tags: {
                ...tags,
                Name: `${name}-db-storage-low`,
            },
        });
    };

    createDatabaseAlarms();

    // // ElastiCache Redis Monitoring
    // const createCacheAlarms = () => {
    //     // Cache CPU
    //     new aws.cloudwatch.MetricAlarm(`${name}-cache-cpu-high`, {
    //         name: `${name}-cache-cpu-high`,
    //         comparisonOperator: "GreaterThanThreshold",
    //         evaluationPeriods: 2,
    //         metricName: "CPUUtilization",
    //         namespace: "AWS/ElastiCache",
    //         period: 300,
    //         statistic: "Average",
    //         threshold: 75,
    //         alarmDescription: "High CPU utilization on Redis cache",
    //         alarmActions: [alertsTopic.arn],
    //         dimensions: {
    //             CacheClusterId: cache.cluster.clusterId,
    //         },
    //         tags: {
    //             ...tags,
    //             Name: `${name}-cache-cpu-high`,
    //         },
    //     });

    //     // Cache memory usage
    //     new aws.cloudwatch.MetricAlarm(`${name}-cache-memory-high`, {
    //         name: `${name}-cache-memory-high`,
    //         comparisonOperator: "GreaterThanThreshold",
    //         evaluationPeriods: 2,
    //         metricName: "DatabaseMemoryUsagePercentage",
    //         namespace: "AWS/ElastiCache",
    //         period: 300,
    //         statistic: "Average",
    //         threshold: 85,
    //         alarmDescription: "High memory usage on Redis cache",
    //         alarmActions: [alertsTopic.arn],
    //         dimensions: {
    //             CacheClusterId: cache.cluster.clusterId,
    //         },
    //         tags: {
    //             ...tags,
    //             Name: `${name}-cache-memory-high`,
    //         },
    //     });

    //     // Cache evictions
    //     new aws.cloudwatch.MetricAlarm(`${name}-cache-evictions`, {
    //         name: `${name}-cache-evictions`,
    //         comparisonOperator: "GreaterThanThreshold",
    //         evaluationPeriods: 2,
    //         metricName: "Evictions",
    //         namespace: "AWS/ElastiCache",
    //         period: 300,
    //         statistic: "Sum",
    //         threshold: 100,
    //         alarmDescription: "High number of evictions on Redis cache",
    //         alarmActions: [alertsTopic.arn],
    //         dimensions: {
    //             CacheClusterId: cache.cluster.clusterId,
    //         },
    //         tags: {
    //             ...tags,
    //             Name: `${name}-cache-evictions`,
    //         },
    //     });
    // };

    // createCacheAlarms();

    // ECS Fargate Monitoring (Aux Agent)
    const createEcsAlarms = () => {
        // Service CPU utilization
        new aws.cloudwatch.MetricAlarm(`${name}-aux-agent-cpu-high`, {
            name: `${name}-aux-agent-cpu-high`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "CPUUtilization",
            namespace: "AWS/ECS",
            period: 300,
            statistic: "Average",
            threshold: 80,
            alarmDescription: "High CPU utilization on aux agent",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                ServiceName: instances.auxAgent.service.name,
                ClusterName: cluster.cluster.name,
            },
            tags: {
                ...tags,
                Name: `${name}-aux-agent-cpu-high`,
            },
        });

        // Service memory utilization
        new aws.cloudwatch.MetricAlarm(`${name}-aux-agent-memory-high`, {
            name: `${name}-aux-agent-memory-high`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "MemoryUtilization",
            namespace: "AWS/ECS",
            period: 300,
            statistic: "Average",
            threshold: 85,
            alarmDescription: "High memory utilization on aux agent",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                ServiceName: instances.auxAgent.service.name,
                ClusterName: cluster.cluster.name,
            },
            tags: {
                ...tags,
                Name: `${name}-aux-agent-memory-high`,
            },
        });

        // Service running count
        new aws.cloudwatch.MetricAlarm(`${name}-aux-agent-not-running`, {
            name: `${name}-aux-agent-not-running`,
            comparisonOperator: "LessThanThreshold",
            evaluationPeriods: 2,
            metricName: "RunningTaskCount",
            namespace: "AWS/ECS",
            period: 300,
            statistic: "Average",
            threshold: 1,
            alarmDescription: "Aux agent service not running",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                ServiceName: instances.auxAgent.service.name,
                ClusterName: cluster.cluster.name,
            },
            tags: {
                ...tags,
                Name: `${name}-aux-agent-not-running`,
            },
        });
    };

    createEcsAlarms();

    // Custom Broker Application Metrics
    const createApplicationAlarms = () => {
        // Broker error rate (custom metric)
        new aws.cloudwatch.MetricAlarm(`${name}-broker-error-rate`, {
            name: `${name}-broker-error-rate`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "broker_error_count",
            namespace: "Boundless/Broker",
            period: 300,
            statistic: "Sum",
            threshold: 10,
            alarmDescription: "High error rate in broker application",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                Environment: name,
                Component: "broker",
            },
            tags: {
                ...tags,
                Name: `${name}-broker-error-rate`,
            },
        });

        // Order processing failures
        new aws.cloudwatch.MetricAlarm(`${name}-order-failures`, {
            name: `${name}-order-failures`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 2,
            metricName: "order_failure_count",
            namespace: "Boundless/Broker",
            period: 300,
            statistic: "Sum",
            threshold: 5,
            alarmDescription: "High number of order processing failures",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                Environment: name,
                Component: "broker",
            },
            tags: {
                ...tags,
                Name: `${name}-order-failures`,
            },
        });

        // Proof generation timeout
        new aws.cloudwatch.MetricAlarm(`${name}-proof-timeouts`, {
            name: `${name}-proof-timeouts`,
            comparisonOperator: "GreaterThanThreshold",
            evaluationPeriods: 1,
            metricName: "proof_timeout_count",
            namespace: "Boundless/Broker",
            period: 600,
            statistic: "Sum",
            threshold: 2,
            alarmDescription: "Proof generation timeouts detected",
            alarmActions: [alertsTopic.arn],
            dimensions: {
                Environment: name,
                Component: "proving",
            },
            tags: {
                ...tags,
                Name: `${name}-proof-timeouts`,
            },
        });
    };

    createApplicationAlarms();

    // Create CloudWatch Dashboard
    const dashboard = new aws.cloudwatch.Dashboard(`${name}-dashboard`, {
        dashboardName: `${name}-bento-cluster`,
        dashboardBody: JSON.stringify({
            widgets: [
                // TODO: Add ECS service metrics instead of EC2 instance metrics
                // {
                //     type: "metric",
                //     x: 0,
                //     y: 0,
                //     width: 12,
                //     height: 6,
                //     properties: {
                //         metrics: [
                //             ["AWS/EC2", "CPUUtilization", "InstanceId", instances.execAgents.instance.id],
                //             [".", ".", ".", instances.snarkAgent.instance.id],
                //             ...instances.gpuProvers.instances.map((instance: any, i: number) => 
                //                 [".", ".", ".", instance.id]
                //             ),
                //         ],
                //         period: 300,
                //         stat: "Average",
                //         region: "us-west-2",
                //         title: "EC2 CPU Utilization",
                //         yAxis: {
                //             left: {
                //                 min: 0,
                //                 max: 100,
                //             },
                //         },
                //     },
                // },
                {
                    type: "metric",
                    x: 0,
                    y: 6,
                    width: 6,
                    height: 6,
                    properties: {
                        metrics: [
                            ["AWS/RDS", "CPUUtilization", "DBInstanceIdentifier", database.instance.id],
                            [".", "DatabaseConnections", ".", "."],
                        ],
                        period: 300,
                        stat: "Average",
                        region: region.name,
                        title: "RDS Performance",
                    },
                },
                // {
                //     type: "metric",
                //     x: 6,
                //     y: 6,
                //     width: 6,
                //     height: 6,
                //     properties: {
                //         metrics: [
                //             ["AWS/ElastiCache", "CPUUtilization", "CacheClusterId", cache.cluster.clusterId],
                //             [".", "DatabaseMemoryUsagePercentage", ".", "."],
                //             [".", "Evictions", ".", "."],
                //         ],
                //         period: 300,
                //         stat: "Average",
                //         region: region.name,
                //         title: "ElastiCache Performance",
                //     },
                // },
                {
                    type: "log",
                    x: 0,
                    y: 12,
                    width: 24,
                    height: 6,
                    properties: {
                        query: `SOURCE '/aws/ec2/${name}-exec-agents' | fields @timestamp, @message\n| filter @message like /ERROR/\n| sort @timestamp desc\n| limit 100`,
                        region: region.name,
                        title: "Recent Errors",
                    },
                },
            ],
        }),
    });

    return {
        alertsTopic,
        dashboard,
        alertsTopicArn: alertsTopic.arn,
    };
}