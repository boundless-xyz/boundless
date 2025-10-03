import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { IndexerShared } from './indexer-infra';

export interface MarketIndexerArgs {
  infra: IndexerShared;
  privSubNetIds: pulumi.Output<string[]>;
  ciCacheSecret?: pulumi.Output<string>;
  githubTokenSecret?: pulumi.Output<string>;
  dockerDir: string;
  dockerTag: string;
  boundlessAddress: string;
  ethRpcUrl: pulumi.Output<string>;
  startBlock: string;
  serviceMetricsNamespace: string;
  boundlessAlertsTopicArns?: string[];
  dockerRemoteBuilder?: string;
}

export class MarketIndexer extends pulumi.ComponentResource {
  constructor(name: string, args: MarketIndexerArgs, opts?: pulumi.ComponentResourceOptions) {
    super('indexer:market', name, opts);

    const {
      infra,
      privSubNetIds,
      ciCacheSecret,
      githubTokenSecret,
      dockerDir,
      dockerTag,
      boundlessAddress,
      ethRpcUrl,
      startBlock,
      serviceMetricsNamespace,
      boundlessAlertsTopicArns,
      dockerRemoteBuilder,
    } = args;

    const serviceName = name;

    let buildSecrets: Record<string, pulumi.Input<string>> = {};
    if (ciCacheSecret !== undefined) {
      const cacheFileData = ciCacheSecret.apply((filePath: any) => fs.readFileSync(filePath, 'utf8'));
      buildSecrets = {
        ci_cache_creds: cacheFileData,
      };
    }
    if (githubTokenSecret !== undefined) {
      buildSecrets = {
        ...buildSecrets,
        githubTokenSecret,
      };
    }

    const marketImage = new docker_build.Image(`${serviceName}-market-img`, {
      tags: [pulumi.interpolate`${infra.ecrRepository.repository.repositoryUrl}:market-${dockerTag}`],
      context: {
        location: dockerDir,
      },
      platforms: ['linux/amd64'],
      push: true,
      dockerfile: {
        location: `${dockerDir}/dockerfiles/market-indexer.dockerfile`,
      },
      builder: dockerRemoteBuilder
        ? {
          name: dockerRemoteBuilder,
        }
        : undefined,
      buildArgs: {
        S3_CACHE_PREFIX: 'private/boundless/rust-cache-docker-Linux-X64/sccache',
      },
      secrets: buildSecrets,
      cacheFrom: [
        {
          registry: {
            ref: pulumi.interpolate`${infra.ecrRepository.repository.repositoryUrl}:cache`,
          },
        },
      ],
      cacheTo: [
        {
          registry: {
            mode: docker_build.CacheMode.Max,
            imageManifest: true,
            ociMediaTypes: true,
            ref: pulumi.interpolate`${infra.ecrRepository.repository.repositoryUrl}:cache`,
          },
        },
      ],
      registries: [
        {
          address: infra.ecrRepository.repository.repositoryUrl,
          password: infra.ecrAuthToken.apply((authToken) => authToken.password),
          username: infra.ecrAuthToken.apply((authToken) => authToken.userName),
        },
      ],
    }, { parent: this });

    const serviceLogGroupName = `${serviceName}-service`;
    const serviceLogGroup = pulumi.output(aws.cloudwatch.getLogGroup({
      name: serviceLogGroupName,
    }, { async: true }).catch(() => {
      return new aws.cloudwatch.LogGroup(serviceLogGroupName, {
        name: serviceLogGroupName,
        retentionInDays: 0,
        skipDestroy: true,
      }, { parent: this });
    }));

    const marketService = new awsx.ecs.FargateService(`${serviceName}-market-service`, {
      name: `${serviceName}-market-service`,
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
          existing: serviceLogGroup,
        },
        executionRole: { roleArn: infra.executionRole.arn },
        taskRole: { roleArn: infra.taskRole.arn },
        container: {
          name: `${serviceName}-market`,
          image: marketImage.ref,
          cpu: 1024,
          memory: 512,
          essential: true,
          linuxParameters: {
            initProcessEnabled: true,
          },
          command: [
            '--rpc-url',
            ethRpcUrl,
            '--boundless-market-address',
            boundlessAddress,
            '--start-block',
            startBlock,
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
              value: '5',
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
    const logGroupArn = pulumi.interpolate`arn:aws:logs:${region}:${accountId}:log-group:${serviceLogGroupName}:*`;

    new aws.iam.RolePolicy(`${serviceName}-market-logs-policy`, {
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

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-market-log-err-filter`, {
      name: `${serviceName}-market-log-err-filter`,
      logGroupName: serviceLogGroupName,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: `${serviceName}-market-log-err`,
        value: '1',
        defaultValue: '0',
      },
      pattern: `"ERROR "`,
    }, { parent: this, dependsOn: [marketService] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-market-error-alarm`, {
      name: `${serviceName}-market-log-err`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: `${serviceName}-market-log-err`,
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
      alarmDescription: 'Market indexer log ERROR level',
      actionsEnabled: true,
      alarmActions,
    }, { parent: this });

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-market-log-fatal-filter`, {
      name: `${serviceName}-market-log-fatal-filter`,
      logGroupName: serviceLogGroupName,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: `${serviceName}-market-log-fatal`,
        value: '1',
        defaultValue: '0',
      },
      pattern: 'FATAL',
    }, { parent: this, dependsOn: [marketService] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-market-fatal-alarm`, {
      name: `${serviceName}-market-log-fatal`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: `${serviceName}-market-log-fatal`,
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
      alarmDescription: `Market indexer ${name} FATAL (task exited)`,
      actionsEnabled: true,
      alarmActions,
    }, { parent: this });

    this.registerOutputs({});
  }
}
