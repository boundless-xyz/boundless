import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import { IndexerShared } from './indexer-infra';

export interface RewardsIndexerArgs {
  infra: IndexerShared;
  privSubNetIds: pulumi.Output<string[]>;
  ethRpcUrl: pulumi.Output<string>;
  vezkcAddress: string;
  zkcAddress: string;
  povwAccountingAddress: string;
  serviceMetricsNamespace: string;
  boundlessAlertsTopicArns?: string[];
}

export class RewardsIndexer extends pulumi.ComponentResource {
  constructor(name: string, args: RewardsIndexerArgs, opts?: pulumi.ComponentResourceOptions) {
    super('indexer:rewards', name, opts);

    const {
      infra,
      privSubNetIds,
      ethRpcUrl,
      vezkcAddress,
      zkcAddress,
      povwAccountingAddress,
      serviceMetricsNamespace,
      boundlessAlertsTopicArns,
    } = args;

    const serviceName = name;

    // Use the unified indexer image from shared infrastructure
    const indexerImage = infra.indexerImage;

    const rewardsServiceLogGroup = `${serviceName}-rewards-service-v2`;

    const rewardsService = new awsx.ecs.FargateService(`${serviceName}-rewards-service`, {
      name: `${serviceName}-rewards-service`,
      cluster: infra.cluster.arn,
      networkConfiguration: {
        securityGroups: [infra.indexerSecurityGroup.id],
        assignPublicIp: false,
        subnets: privSubNetIds,
      },
      desiredCount: 1,
      deploymentCircuitBreaker: {
        enable: false,
        rollback: false,
      },
      forceNewDeployment: true,
      enableExecuteCommand: true,
      taskDefinitionArgs: {
        logGroup: {
          args: {
            name: rewardsServiceLogGroup,
            retentionInDays: 0,
            skipDestroy: true,
          },
        },
        executionRole: { roleArn: infra.executionRole.arn },
        taskRole: { roleArn: infra.taskRole.arn },
        container: {
          name: `${serviceName}-rewards`,
          image: indexerImage.ref,
          cpu: 512,
          memory: 256,
          essential: true,
          linuxParameters: {
            initProcessEnabled: true,
          },
          command: [
            './rewards-indexer',
            '--rpc-url',
            ethRpcUrl,
            '--vezkc-address',
            vezkcAddress,
            '--zkc-address',
            zkcAddress,
            '--povw-accounting-address',
            povwAccountingAddress,
            '--log-json',
          ],
          secrets: [
            {
              name: 'DATABASE_URL',
              valueFrom: infra.dbUrlSecret.arn,
            },
          ],
          environment: [
            {
              name: 'RUST_LOG',
              value: 'boundless_indexer=debug,info',
            },
            {
              name: 'NO_COLOR',
              value: '1',
            },
            {
              name: 'RUST_BACKTRACE',
              value: '1',
            },
            {
              name: 'DB_POOL_SIZE',
              value: '3',
            },
            {
              name: 'SECRET_HASH',
              value: infra.secretHash,
            },
          ],
        },
      },
    }, { parent: this, dependsOn: [infra.taskRole, infra.taskRolePolicyAttachment] });

    // Grant execution role permission to write to this service's specific log group
    const region = aws.getRegionOutput().name;
    const accountId = aws.getCallerIdentityOutput().accountId;
    const logGroupArn = pulumi.interpolate`arn:aws:logs:${region}:${accountId}:log-group:${rewardsServiceLogGroup}:*`;

    new aws.iam.RolePolicy(`${serviceName}-rewards-logs-policy`, {
      role: infra.executionRole.id,
      policy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['logs:CreateLogStream', 'logs:PutLogEvents'],
            Resource: logGroupArn,
          },
        ],
      },
    }, { parent: this });

    const alarmActions = boundlessAlertsTopicArns ?? [];

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-rewards-log-err-filter`, {
      name: `${serviceName}-rewards-log-err-filter`,
      logGroupName: rewardsServiceLogGroup,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: `${serviceName}-rewards-log-err`,
        value: '1',
        defaultValue: '0',
      },
      pattern: `"ERROR "`,
    }, { parent: this, dependsOn: [rewardsService] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-rewards-error-alarm`, {
      name: `${serviceName}-rewards-log-err`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: `${serviceName}-rewards-log-err`,
            period: 60,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 1,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 60,
      datapointsToAlarm: 2,
      treatMissingData: 'notBreaching',
      alarmDescription: 'Rewards indexer log ERROR level',
      actionsEnabled: true,
      alarmActions,
    }, { parent: this });

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-rewards-log-fatal-filter`, {
      name: `${serviceName}-rewards-log-fatal-filter`,
      logGroupName: rewardsServiceLogGroup,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: `${serviceName}-rewards-log-fatal`,
        value: '1',
        defaultValue: '0',
      },
      pattern: 'FATAL',
    }, { parent: this, dependsOn: [rewardsService] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-rewards-fatal-alarm`, {
      name: `${serviceName}-rewards-log-fatal`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: `${serviceName}-rewards-log-fatal`,
            period: 60,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 1,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 1,
      datapointsToAlarm: 1,
      treatMissingData: 'notBreaching',
      alarmDescription: `Rewards indexer ${name} FATAL (task exited)`,
      actionsEnabled: true,
      alarmActions,
    }, { parent: this });

    this.registerOutputs({});
  }
}
