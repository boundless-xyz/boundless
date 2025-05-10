import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { getEnvVar, ChainId, getServiceNameV1, Severity } from "../util";
import { create } from 'domain';

require('dotenv').config();

export = () => {
  // Read config
  const config = new pulumi.Config();

  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const serviceName = getServiceNameV1(stackName, "bonsai-prover", ChainId.SEPOLIA);
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;

  const privateKey = isDev ? getEnvVar("PRIVATE_KEY") : config.requireSecret('PRIVATE_KEY');
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');
  const orderStreamUrl = isDev ? getEnvVar("ORDER_STREAM_URL") : config.requireSecret('ORDER_STREAM_URL');

  const baseStackName = config.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');

  const setVerifierAddr = config.require('SET_VERIFIER_ADDR');
  const proofMarketAddr = config.require('PROOF_MARKET_ADDR');
  
  const bonsaiApiUrl = config.require('BONSAI_API_URL');
  const bonsaiApiKey = isDev ? getEnvVar("BONSAI_API_KEY") : config.getSecret('BONSAI_API_KEY');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  
  const brokerTomlPath = config.require('BROKER_TOML_PATH')
  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');

  const ethRpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerEthRpc`);
  const _ethRpcUrlSecretSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-brokerEthRpc`, {
    secretId: ethRpcUrlSecret.id,
    secretString: ethRpcUrl,
  });
  
  const privateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-brokerPrivateKey`);
  const _privateKeySecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-privateKeyValue`, {
    secretId: privateKeySecret.id,
    secretString: privateKey,
  });

  const bonsaiSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerBonsaiKey`);
  const _bonsaiSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-bonsaiKeyValue`, {
    secretId: bonsaiSecret.id,
    secretString: bonsaiApiKey,
  });

  const orderStreamUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerOrderStreamUrl`);
  const _orderStreamUrlSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-brokerOrderStreamUrl`, {
    secretId: orderStreamUrlSecret.id,
    secretString: orderStreamUrl,
  });

  const brokerS3Bucket = new aws.s3.Bucket(serviceName, {
    bucketPrefix: serviceName,
    tags: {
      Name: serviceName,
    },
  });

  const fileToUpload = new pulumi.asset.FileAsset(brokerTomlPath);

  const bucketObject = new aws.s3.BucketObject(serviceName, {
    bucket: brokerS3Bucket.id,
    key: 'broker.toml',
    source: fileToUpload,
  });

  // EFS
  const fileSystem = new aws.efs.FileSystem(`${serviceName}-efs-rev3`, {
    encrypted: true,
    tags: {
      Name: serviceName,
    },
  });

  const mountTargets = privSubNetIds.apply((subnets) =>
    subnets.map((subnetId: string, index: number) => {
      return new aws.efs.MountTarget(`${serviceName}-mount-${index}`, {
        fileSystemId: fileSystem.id,
        subnetId: subnetId,
        securityGroups: [brokerSecGroup.id],
      }, { dependsOn: [fileSystem] });
    })
  );

  const taskRole = new aws.iam.Role(`${serviceName}-task-role`, {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
      Service: 'ecs-tasks.amazonaws.com',
    }),
  });

  const _rolePolicy = new aws.iam.RolePolicy(`${serviceName}-role-policy`, {
    role: taskRole.id,
    policy: {
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Action: ['s3:GetObject', 's3:ListObject', 's3:HeadObject'],
          Resource: [bucketObject.arn],
        },
        {
          Effect: 'Allow',
          Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
          Resource: [privateKeySecret.arn, bonsaiSecret.arn, ethRpcUrlSecret.arn, orderStreamUrlSecret.arn],
        },
      ],
    },
  }, { dependsOn: [taskRole] });

  const brokerEcr = new awsx.ecr.Repository(`${serviceName}-ecr`, {
    lifecyclePolicy: {
      rules: [
        {
          description: 'Delete untagged images after N days',
          tagStatus: 'untagged',
          maximumAgeLimit: 7,
        },
      ],
    },
    forceDelete: true,
  });

  const executionRole = new aws.iam.Role(`${serviceName}-ecs-execution-role`, {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
      Service: 'ecs-tasks.amazonaws.com',
    }),
  });

  pulumi.all([brokerEcr.repository.arn, executionRole.name]).apply(([ecrRepoArn, executionRoleName]) => {
    new aws.iam.RolePolicy(`${serviceName}-ecs-execution-pol`, {
      role: executionRoleName,
      policy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: [
              'ecr:GetAuthorizationToken',
              'ecr:BatchCheckLayerAvailability',
              'ecr:GetDownloadUrlForLayer',
              'ecr:BatchGetImage',
            ],
            Resource: '*',
          },
          {
            Effect: 'Allow',
            Action: [
              'logs:CreateLogStream',
              'logs:PutLogEvents',
            ],
            Resource: '*',
          },
          {
            Effect: 'Allow',
            Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
            Resource: [privateKeySecret.arn, bonsaiSecret.arn, ethRpcUrlSecret.arn, orderStreamUrlSecret.arn],
          },
        ],
      },
    }, { dependsOn: [executionRole] });
  })

  const authToken = aws.ecr.getAuthorizationTokenOutput({
    registryId: brokerEcr.repository.registryId,
  });

  // Optionally add in the gh token secret and sccache s3 creds to the build ctx
  let buildSecrets = {};
  if (ciCacheSecret !== undefined) {
    const cacheFileData = ciCacheSecret.apply((filePath) => fs.readFileSync(filePath, 'utf8'));
    buildSecrets = {
      ci_cache_creds: cacheFileData,
    };
  }
  if (githubTokenSecret !== undefined) {
    buildSecrets = {
      ...buildSecrets,
      githubTokenSecret
    }
  }

  const dockerTagPath = pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:${dockerTag}`;

  const image = new docker_build.Image(serviceName, {
    tags: [dockerTagPath],
    context: {
      location: dockerDir,
    },
    platforms: ['linux/amd64'],
    push: true,
    builder: dockerRemoteBuilder ? {
      name: dockerRemoteBuilder,
    } : undefined,
    dockerfile: {
      location: `${dockerDir}/dockerfiles/${isDev ? 'dev/' : ''}broker.dockerfile`,
    },
    buildArgs: {
      S3_CACHE_PREFIX: 'private/boundless/rust-cache-docker-Linux-X64/sccache',
    },
    secrets: buildSecrets,
    cacheFrom: [
      {
        registry: {
          ref: pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:cache`,
        },
      },
    ],
    cacheTo: [
      {
        registry: {
          mode: docker_build.CacheMode.Max,
          imageManifest: true,
          ociMediaTypes: true,
          ref: pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:cache`,
        },
      },
    ],
    registries: [
      {
        address: brokerEcr.repository.repositoryUrl,
        password: authToken.password,
        username: authToken.userName,
      },
    ],
  });

  const cluster = new aws.ecs.Cluster(serviceName, {
    name: serviceName,
  });

  const brokerSecGroup = new aws.ec2.SecurityGroup(serviceName, {
    name: serviceName,
    vpcId: vpcId,
    egress: [
      {
        fromPort: 0,
        toPort: 0,
        protocol: '-1',
        cidrBlocks: ['0.0.0.0/0'],
        ipv6CidrBlocks: ['::/0'],
      },
    ],
  });

  const _efsInbound = new aws.ec2.SecurityGroupRule(`${serviceName}-efs-inbound`, {
    type: 'ingress',
    fromPort: 2049,
    toPort: 2049,
    protocol: 'tcp',
    securityGroupId: brokerSecGroup.id,
    sourceSecurityGroupId: brokerSecGroup.id,
  });

  const brokerS3BucketName = brokerS3Bucket.bucket.apply(n => n);

  const service = new awsx.ecs.FargateService(serviceName, {
    name: serviceName,
    cluster: cluster.arn,
    networkConfiguration: {
      securityGroups: [brokerSecGroup.id],
      assignPublicIp: false,
      subnets: privSubNetIds,
    },
    deploymentCircuitBreaker: {
      enable: false,
      rollback: true,
    },
    desiredCount: 1,
    // These min/maxs prevent 2 tasks from being run in parallel
    // because broker is a singleton but FARGATE does not support DAEMON strategy
    deploymentMinimumHealthyPercent: 0,
    deploymentMaximumPercent: 100,
    enableExecuteCommand: true,
    taskDefinitionArgs: {
      family: 'broker',
      executionRole: {
        roleArn: executionRole.arn,
      },
      taskRole: {
        roleArn: taskRole.arn,
      },
      logGroup: {
        args: {
          name: serviceName,
          retentionInDays: 0,
          skipDestroy: true,
        },
      },
      volumes: [
        {
          name: 'broker-storage',
          efsVolumeConfiguration: {
            fileSystemId: fileSystem.id,
            rootDirectory: '/',
          },
        },
      ],
      container: {
        name: serviceName,
        image: image.ref,
        mountPoints: [
          {
            sourceVolume: 'broker-storage',
            containerPath: '/app/data',
            readOnly: false,
          },
        ],
        entryPoint: ['/bin/sh', '-c'],
        cpu: 4096,
        memory: 8192,
        essential: true,
        linuxParameters: {
          initProcessEnabled: true,
        },
        command: [
          `/usr/bin/aws s3 cp s3://$BUCKET/broker.toml /app/broker.toml && /app/broker --set-verifier-address ${setVerifierAddr} --boundless-market-address ${proofMarketAddr} --config-file /app/broker.toml --db-url sqlite:///app/data/broker.db`,
        ],
        secrets: [
          {
            name: 'PRIVATE_KEY',
            valueFrom: privateKeySecret.arn,
          },
          {
            name: 'BONSAI_API_KEY',
            valueFrom: bonsaiSecret.arn,
          },
          {
            name: 'RPC_URL',
            valueFrom: ethRpcUrlSecret.arn,
          },
          {
            name: 'ORDER_STREAM_URL',
            valueFrom: orderStreamUrlSecret.arn,
          }
        ],
        environment: [
          { name: 'NO_COLOR', value: '1' },
          { name: 'RUST_LOG', value: 'broker=debug,boundless_market=debug' },
          { name: 'RUST_BACKTRACE', value: '1' },
          { name: 'BONSAI_API_URL', value: bonsaiApiUrl },
          { name: 'BUCKET', value: brokerS3BucketName }
        ],
      },
    },
  }, { dependsOn: [fileSystem,mountTargets] });

  const alarmActions = boundlessAlertsTopicArn ? [boundlessAlertsTopicArn] : [];

  const createErrorCodeAlarm = (
    pattern: string, 
    metricName: string, 
    severity: Severity,
    alarmConfig?: Partial<aws.cloudwatch.MetricAlarmArgs>,
    metricConfig?: Partial<aws.types.input.cloudwatch.MetricAlarmMetricQueryMetric>,
    description?: string
  ): void => {
    // Generate a metric by filtering for the error code
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-${metricName}-${severity}-filter`, {
      name: `${serviceName}-${metricName}-${severity}-filter`,
      logGroupName: serviceName,
      metricTransformation: {
        namespace: `Boundless/Services/${serviceName}`,
        name: `${serviceName}-${metricName}-${severity}`,
        value: '1',
        defaultValue: '0',
      },
      pattern,
    }, { dependsOn: [service] });

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
      alarmDescription: `${severity} ${metricName} ${description}`,
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
  createErrorCodeAlarm('ERROR -"[B-"', 'error-without-code', Severity.SEV2);

  // Alarms for low balances
  createErrorCodeAlarm('WARN "[B-BAL-ETH]"', 'low-balance-alert-eth', Severity.SEV2);
  createErrorCodeAlarm('WARN "[B-BAL-STK]"', 'low-balance-alert-stk', Severity.SEV2);
  createErrorCodeAlarm('ERROR "[B-BAL-ETH]"', 'low-balance-alert-eth', Severity.SEV1);
  createErrorCodeAlarm('ERROR "[B-BAL-STK]"', 'low-balance-alert-stk', Severity.SEV1);
  
  // Alarms at the supervisor level
  //
  // 2 supervisor restarts within 15 mins triggers a SEV2 alarm
  createErrorCodeAlarm('"[B-SUP-RECOVER]"', 'supervisor-recover-errors', Severity.SEV2, {
    threshold: 2,
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

  // 3 errors when fetching images/inputs within 15 minutes triggers a SEV2 alarm.
  // Note: This is a pattern to match "[B-OP-001]" OR "[B-OP-002]"
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

  // Any 1 unexpected error in the submitter triggers a SEV2 alarm.
  createErrorCodeAlarm('"[B-SUB-500]"', 'submitter-unexpected-error', Severity.SEV2);

  // 3 unexpected errors within 5 minutes in the submitter triggers a SEV1 alarm.
  createErrorCodeAlarm('"[B-SUB-500]"', 'submitter-unexpected-error', Severity.SEV1, {
    threshold: 3,
  }, { period: 300 });
  
};
