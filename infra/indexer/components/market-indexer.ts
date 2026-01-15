import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { IndexerShared } from './indexer-infra';
import { Severity } from '../../util';

export interface MarketIndexerArgs {
  infra: IndexerShared;
  privSubNetIds: pulumi.Output<string[]>;
  ciCacheSecret?: pulumi.Output<string>;
  githubTokenSecret?: pulumi.Output<string>;
  dockerDir: string;
  dockerTag: string;
  boundlessAddress: string;
  ethRpcUrl: pulumi.Output<string>;
  logsEthRpcUrl?: pulumi.Output<string>;
  startBlock: string;
  serviceMetricsNamespace: string;
  boundlessAlertsTopicArns?: string[];
  dockerRemoteBuilder?: string;
  orderStreamUrl?: pulumi.Output<string>;
  orderStreamApiKey?: pulumi.Output<string>;
  bentoApiUrl?: pulumi.Output<string>;
  bentoApiKey?: pulumi.Output<string>;
  rustLogLevel: string;
}

export class MarketIndexer extends pulumi.ComponentResource {
  public readonly backfillLambdaName: pulumi.Output<string>;

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
      logsEthRpcUrl,
      startBlock,
      serviceMetricsNamespace,
      boundlessAlertsTopicArns,
      dockerRemoteBuilder,
      orderStreamUrl,
      orderStreamApiKey,
      bentoApiUrl,
      bentoApiKey,
      rustLogLevel,
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

    const marketImage = new docker_build.Image(`${serviceName}-market-img-${infra.databaseVersion}`, {
      tags: [pulumi.interpolate`${infra.ecrRepository.repository.repositoryUrl}:market-${dockerTag}-${infra.databaseVersion}`],
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

    const marketService = new awsx.ecs.FargateService(`${serviceName}-market-indexer-${infra.databaseVersion}`, {
      name: `${serviceName}-market-indexer-${infra.databaseVersion}`,
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
          name: `${serviceName}-market-${infra.databaseVersion}`,
          image: marketImage.ref,
          cpu: 2048,
          memory: 2048,
          essential: true,
          linuxParameters: {
            initProcessEnabled: true,
          },
          command: [
            '--rpc-url',
            ethRpcUrl,
            ...(logsEthRpcUrl ? ['--logs-rpc-url', logsEthRpcUrl] : []),
            '--boundless-market-address',
            boundlessAddress,
            '--start-block',
            startBlock,
            // Note, due to the way the caching works (cache files are stored based on the block range they were queried for),
            // changing this value invalidates old cache entries, and thus will slow down re-indexing
            '--batch-size',
            '9999',
            '--tx-fetch-strategy',
            'tx-by-hash',
            '--log-json',
            '--cache-uri',
            pulumi.interpolate`s3://${infra.cacheBucket.bucket}`,
            ...(orderStreamUrl ? ['--order-stream-url', orderStreamUrl] : []),
            ...(orderStreamApiKey ? ['--order-stream-api-key', orderStreamApiKey] : []),
            ...(bentoApiUrl ? ['--bento-api-url', bentoApiUrl] : []),
            ...(bentoApiKey ? ['--bento-api-key', bentoApiKey] : []),
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
              value: rustLogLevel,
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
            {
              name: 'AWS_REGION',
              value: 'us-west-2',
            },
          ],
        },
      },
    }, { parent: this, dependsOn: [infra.taskRole, infra.taskRolePolicyAttachment] });

    // Backfill infrastructure
    // Build backfill Docker image
    const backfillImage = new docker_build.Image(`${serviceName}-backfill-img-${infra.databaseVersion}`, {
      tags: [pulumi.interpolate`${infra.ecrRepository.repository.repositoryUrl}:market-backfill-${dockerTag}-${infra.databaseVersion}`],
      context: { location: dockerDir },
      platforms: ['linux/amd64'],
      push: true,
      dockerfile: { location: `${dockerDir}/dockerfiles/market-indexer-backfill.dockerfile` },
      builder: dockerRemoteBuilder ? { name: dockerRemoteBuilder } : undefined,
      buildArgs: { S3_CACHE_PREFIX: 'private/boundless/rust-cache-docker-Linux-X64/sccache' },
      secrets: buildSecrets,
      cacheFrom: [{ registry: { ref: pulumi.interpolate`${infra.ecrRepository.repository.repositoryUrl}:cache` } }],
      cacheTo: [{ registry: { mode: docker_build.CacheMode.Max, imageManifest: true, ociMediaTypes: true, ref: pulumi.interpolate`${infra.ecrRepository.repository.repositoryUrl}:cache` } }],
      registries: [{ address: infra.ecrRepository.repository.repositoryUrl, password: infra.ecrAuthToken.apply(t => t.password), username: infra.ecrAuthToken.apply(t => t.userName) }],
    }, { parent: this });

    // Create dedicated log group for backfill
    const backfillLogGroupName = `${serviceName}-backfill`;
    const backfillLogGroup = new aws.cloudwatch.LogGroup(backfillLogGroupName, {
      name: backfillLogGroupName,
      retentionInDays: 0,
      skipDestroy: true,
    }, { parent: this });

    // Grant execution role permission to write to backfill log group
    const region = aws.getRegionOutput().name;
    const accountId = aws.getCallerIdentityOutput().accountId;
    const backfillLogGroupArn = pulumi.interpolate`arn:aws:logs:${region}:${accountId}:log-group:${backfillLogGroupName}:*`;

    new aws.iam.RolePolicy(`${serviceName}-backfill-logs-policy`, {
      role: infra.executionRole.id,
      policy: {
        Version: '2012-10-17',
        Statement: [{
          Effect: 'Allow',
          Action: ['logs:CreateLogStream', 'logs:PutLogEvents'],
          Resource: backfillLogGroupArn,
        }],
      },
    }, { parent: this });

    // Create backfill task definition
    const backfillContainerName = `${serviceName}-backfill-${infra.databaseVersion}`;
    const backfillTaskDef = new awsx.ecs.FargateTaskDefinition(`${serviceName}-backfill-task-${infra.databaseVersion}`, {
      family: `${serviceName}-market-backfill-${infra.databaseVersion}`,
      logGroup: { existing: backfillLogGroup },
      executionRole: { roleArn: infra.executionRole.arn },
      taskRole: { roleArn: infra.taskRole.arn },
      container: {
        name: backfillContainerName,
        image: backfillImage.ref,
        cpu: 2048,
        memory: 2048,
        essential: true,
        linuxParameters: { initProcessEnabled: true },
        secrets: [{ name: 'DATABASE_URL', valueFrom: infra.dbUrlSecret.arn }],
        environment: [
          { name: 'RUST_LOG', value: 'boundless_indexer=debug,info' },
          { name: 'NO_COLOR', value: '1' },
          { name: 'RUST_BACKTRACE', value: '1' },
          { name: 'DB_POOL_SIZE', value: '5' },
          { name: 'SECRET_HASH', value: infra.secretHash },
          { name: 'AWS_REGION', value: 'us-west-2' },
        ],
      },
    }, { parent: this, dependsOn: [infra.taskRole, infra.taskRolePolicyAttachment] });

    // Create Lambda IAM role
    const lambdaRole = new aws.iam.Role(`${serviceName}-backfill-lambda-role`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({ Service: 'lambda.amazonaws.com' }),
      managedPolicyArns: [aws.iam.ManagedPolicy.AWSLambdaBasicExecutionRole],
    }, { parent: this });

    new aws.iam.RolePolicy(`${serviceName}-backfill-lambda-policy`, {
      role: lambdaRole.id,
      policy: pulumi.all([backfillTaskDef.taskDefinition.arn, infra.executionRole.arn, infra.taskRole.arn]).apply(
        ([taskDefArn, execRoleArn, taskRoleArn]) => JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Action: ['ecs:RunTask'],
              Resource: taskDefArn.replace(/:\d+$/, ':*'), // Allow any revision
            },
            {
              Effect: 'Allow',
              Action: ['iam:PassRole'],
              Resource: [execRoleArn, taskRoleArn],
            },
          ],
        })
      ),
    }, { parent: this });

    // Create Lambda function
    const backfillLambda = new aws.lambda.Function(`${serviceName}-backfill-start`, {
      name: `${serviceName}-backfill-start`,
      role: lambdaRole.arn,
      runtime: 'nodejs20.x',
      handler: 'index.handler',
      timeout: 30,
      code: new pulumi.asset.AssetArchive({
        '.': new pulumi.asset.FileArchive('../indexer/backfill-trigger-lambda/build'),
      }),
      environment: {
        variables: {
          CLUSTER_ARN: infra.cluster.arn,
          TASK_DEFINITION_ARN: backfillTaskDef.taskDefinition.arn,
          SUBNET_IDS: privSubNetIds.apply(ids => ids.join(',')),
          SECURITY_GROUP_ID: infra.indexerSecurityGroup.id,
          CONTAINER_NAME: backfillContainerName,
          RPC_URL: ethRpcUrl,
          LOGS_RPC_URL: logsEthRpcUrl ?? ethRpcUrl,
          BOUNDLESS_ADDRESS: boundlessAddress,
          CACHE_BUCKET: infra.cacheBucket.bucket,
          // Scheduled backfill configuration
          SCHEDULED_BACKFILL_MODE: 'statuses_and_aggregates', // Default to aggregates for daily runs
          START_BLOCK: startBlock, // Use same start block as regular indexer
        },
      },
    }, { parent: this, dependsOn: [lambdaRole] });

    // Export Lambda name for easy reference
    this.backfillLambdaName = backfillLambda.name;

    // Create EventBridge rule for daily scheduled backfill
    const backfillScheduleRule = new aws.cloudwatch.EventRule(`${serviceName}-backfill-rule`, {
      name: `${serviceName}-backfill-rule`,
      description: `Daily scheduled backfill for ${serviceName}`,
      scheduleExpression: 'cron(0 2 * * ? *)', // Run daily at 2 AM UTC
      state: 'ENABLED',
    }, {
      parent: this,
    });

    // Grant EventBridge permission to invoke Lambda
    new aws.lambda.Permission(`${serviceName}-backfill-lmbd-perm`, {
      statementId: `AllowEventBridge-${serviceName}`,
      action: 'lambda:InvokeFunction',
      function: backfillLambda.name,
      principal: 'events.amazonaws.com',
      sourceArn: backfillScheduleRule.arn,
    }, { parent: this });

    // Add Lambda as target for EventBridge rule
    new aws.cloudwatch.EventTarget(`${serviceName}-backfill-targ`, {
      rule: backfillScheduleRule.name,
      arn: backfillLambda.arn,
    }, { parent: this });

    // Grant execution role permission to write to this service's specific log group
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

    const errorLogMetricName = `${serviceName}-market-log-err`;
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-market-log-err-filter`, {
      name: `${serviceName}-market-log-err-filter`,
      logGroupName: serviceLogGroupName,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: errorLogMetricName,
        value: '1',
        defaultValue: '0',
      },
      pattern: `"ERROR "`,
    }, { parent: this, dependsOn: [marketService] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-market-error-alarm-${Severity.SEV2}`, {
      name: `${serviceName}-market-log-err-${Severity.SEV2}`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: errorLogMetricName,
            period: 300,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 2,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 12,
      datapointsToAlarm: 3,
      treatMissingData: 'notBreaching',
      alarmDescription: `Market indexer ${name}: ERROR 3 datapoints within 1 hour ${Severity.SEV2}`,
      actionsEnabled: true,
      alarmActions,
    }, { parent: this });

    const fatalLogMetricName = `${serviceName}-market-log-fatal`;
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-market-log-fatal-filter`, {
      name: `${serviceName}-market-log-fatal-filter`,
      logGroupName: serviceLogGroupName,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: fatalLogMetricName,
        value: '1',
        defaultValue: '0',
      },
      pattern: 'FATAL',
    }, { parent: this, dependsOn: [marketService] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-market-fatal-alarm-${Severity.SEV2}`, {
      name: `${serviceName}-market-log-fatal-${Severity.SEV2}`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: fatalLogMetricName,
            period: 300,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 1,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 12,
      datapointsToAlarm: 3,
      treatMissingData: 'notBreaching',
      alarmDescription: `Market indexer ${name} FATAL 3 times within 1 hour ${Severity.SEV2}`,
      actionsEnabled: true,
      alarmActions,
    }, { parent: this });

    const panickedLogMetricName = `${serviceName}-market-log-panicked`;
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-market-log-panicked-filter`, {
      name: `${serviceName}-market-log-panicked-filter`,
      logGroupName: serviceLogGroupName,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: panickedLogMetricName,
        value: '1',
        defaultValue: '0',
      },
      pattern: 'panicked',
    }, { parent: this, dependsOn: [marketService] });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-market-panicked-alarm-${Severity.SEV2}`, {
      name: `${serviceName}-market-log-panicked-${Severity.SEV2}`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: panickedLogMetricName,
            period: 300,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 1,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 12,
      datapointsToAlarm: 3,
      treatMissingData: 'notBreaching',
      alarmDescription: `Market indexer ${name} PANICKED 3 times within 1 hour ${Severity.SEV2}`,
      actionsEnabled: true,
      alarmActions,
    }, { parent: this });

    this.registerOutputs({});
  }
}
