import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import * as docker_build from '@pulumi/docker-build';
import { getServiceNameV1, getEnvVar, Severity } from '../util';
import * as crypto from 'crypto';
require('dotenv').config();

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const chainId = config.require('CHAIN_ID');
  const serviceName = getServiceNameV1(stackName, "distributor", chainId);

  const privateKey = isDev ? getEnvVar("DISTRIBUTOR_PRIVATE_KEY") : config.requireSecret('DISTRIBUTOR_PRIVATE_KEY');
  const distributorAddress = isDev ? getEnvVar("DISTRIBUTOR_ADDRESS") : config.require('DISTRIBUTOR_ADDRESS');
  const slasherKey = isDev ? getEnvVar("SLASHER_KEY") : config.requireSecret('SLASHER_KEY');
  const proverKeys = isDev ? getEnvVar("PROVER_KEYS") : config.requireSecret('PROVER_KEYS');
  const orderGeneratorKeys = isDev ? getEnvVar("ORDER_GENERATOR_KEYS") : config.requireSecret('ORDER_GENERATOR_KEYS');
  const offchainRequestorAddresses = isDev ? getEnvVar("OFFCHAIN_REQUESTOR_ADDRESSES") : config.get('OFFCHAIN_REQUESTOR_ADDRESSES');

  const ethThreshold = isDev ? getEnvVar("ETH_THRESHOLD") : config.require('ETH_THRESHOLD');
  const stakeThreshold = isDev ? getEnvVar("STAKE_THRESHOLD") : config.require('STAKE_THRESHOLD');
  const ethTopUpAmount = isDev ? getEnvVar("ETH_TOP_UP_AMOUNT") : config.require('ETH_TOP_UP_AMOUNT');
  const stakeTopUpAmount = isDev ? getEnvVar("STAKE_TOP_UP_AMOUNT") : config.require('STAKE_TOP_UP_AMOUNT');
  const proverEthDonateThreshold = isDev ? getEnvVar("PROVER_ETH_DONATE_THRESHOLD") : config.require('PROVER_ETH_DONATE_THRESHOLD');
  const proverStakeDonateThreshold = isDev ? getEnvVar("PROVER_STAKE_DONATE_THRESHOLD") : config.require('PROVER_STAKE_DONATE_THRESHOLD');
  const distributorEthAlertThreshold = config.get('DISTRIBUTOR_ETH_ALERT_THRESHOLD');
  const distributorStakeAlertThreshold = config.get('DISTRIBUTOR_STAKE_ALERT_THRESHOLD');

  // External prover top-up config (all optional)
  const enableExternalTopup = config.getBoolean('ENABLE_EXTERNAL_TOPUP') ?? false;
  const indexerApiUrl = enableExternalTopup ? config.requireSecret('INDEXER_API_URL') : config.get('INDEXER_API_URL');
  const minBcyclesThreshold = config.get('MIN_BCYCLES_THRESHOLD');
  const externalCollateralThreshold = config.get('EXTERNAL_COLLATERAL_THRESHOLD');
  const externalPerTopUpAmount = config.get('EXTERNAL_PER_TOP_UP_AMOUNT');
  const externalLifetimeAllowance = config.get('EXTERNAL_LIFETIME_ALLOWANCE');
  const chainalysisOracleAddress = config.get('CHAINALYSIS_ORACLE_ADDRESS');

  const scheduleMinutes = config.require('SCHEDULE_MINUTES');

  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;

  const logLevel = config.require('LOG_LEVEL');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');

  const boundlessMarketAddr = config.get('BOUNDLESS_MARKET_ADDR');
  const setVerifierAddr = config.get('SET_VERIFIER_ADDR');
  const collateralTokenAddr = config.get('COLLATERAL_TOKEN_ADDR');

  const githubTokenSecret = config.get('GH_TOKEN_SECRET');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');

  const baseStackName = config.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');

  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');
  const boundlessPagerdutyTopicArn = config.get('PAGERDUTY_ALERTS_TOPIC_ARN');
  const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];

  const distributorPrivateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-distributor-private-key`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-distributor-private-key`, {
    secretId: distributorPrivateKeySecret.id,
    secretString: privateKey,
  });

  const rpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-rpc-url`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-rpc-url`, {
    secretId: rpcUrlSecret.id,
    secretString: ethRpcUrl,
  });

  const slasherKeySecret = new aws.secretsmanager.Secret(`${serviceName}-slasher-key`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-slasher-key`, {
    secretId: slasherKeySecret.id,
    secretString: slasherKey,
  });

  const proverKeysSecret = new aws.secretsmanager.Secret(`${serviceName}-prover-keys`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-prover-keys`, {
    secretId: proverKeysSecret.id,
    secretString: proverKeys,
  });

  const orderGeneratorKeysSecret = new aws.secretsmanager.Secret(`${serviceName}-order-generator-keys`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-order-generator-keys`, {
    secretId: orderGeneratorKeysSecret.id,
    secretString: orderGeneratorKeys,
  });

  const indexerApiUrlSecret = indexerApiUrl
    ? (() => {
        const secret = new aws.secretsmanager.Secret(`${serviceName}-indexer-api-url`);
        new aws.secretsmanager.SecretVersion(`${serviceName}-indexer-api-url`, {
          secretId: secret.id,
          secretString: indexerApiUrl,
        });
        return secret;
      })()
    : undefined;

  const secretHash = pulumi
    .all([ethRpcUrl, privateKey])
    .apply(([_ethRpcUrl, _privateKey]: [string, string]) => {
      const hash = crypto.createHash("sha1");
      hash.update(_ethRpcUrl);
      hash.update(_privateKey);
      return hash.digest("hex");
    });

  const repo = new awsx.ecr.Repository(`${serviceName}-ecr-repo`, {
    name: `${serviceName}-ecr-repo`,
    forceDelete: true,
    lifecyclePolicy: {
      rules: [
        {
          description: 'Delete untagged images after N days',
          tagStatus: 'untagged',
          maximumAgeLimit: 7,
        },
      ],
    },
  });

  const authToken = aws.ecr.getAuthorizationTokenOutput({
    registryId: repo.repository.registryId,
  });

  const dockerTagPath = pulumi.interpolate`${repo.repository.repositoryUrl}:${dockerTag}`;

  // Optionally add in the gh token secret and sccache s3 creds to the build ctx
  let buildSecrets = {};
  if (ciCacheSecret !== undefined) {
    buildSecrets = {
      ci_cache_creds: ciCacheSecret,
    };
  }
  if (githubTokenSecret !== undefined) {
    buildSecrets = {
      ...buildSecrets,
      githubTokenSecret
    }
  }

  const image = new docker_build.Image(`${serviceName}-image`, {
    tags: [dockerTagPath],
    context: {
      location: dockerDir,
    },
    // Due to limitations with cargo-chef, we need to build for amd64, even though distributor doesn't
    // strictly need r0vm. See `dockerfiles/distributor.dockerfile` for more details.
    platforms: ['linux/amd64'],
    secrets: buildSecrets,
    push: true,
    builder: dockerRemoteBuilder ? {
      name: dockerRemoteBuilder,
    } : undefined,
    dockerfile: {
      location: `${dockerDir}/dockerfiles/distributor.dockerfile`,
    },
    cacheFrom: [
      {
        registry: {
          ref: pulumi.interpolate`${repo.repository.repositoryUrl}:cache`,
        },
      },
    ],
    cacheTo: [
      {
        registry: {
          mode: docker_build.CacheMode.Max,
          imageManifest: true,
          ociMediaTypes: true,
          ref: pulumi.interpolate`${repo.repository.repositoryUrl}:cache`,
        },
      },
    ],
    registries: [
      {
        address: repo.repository.repositoryUrl,
        password: authToken.password,
        username: authToken.userName,
      },
    ],
  });

  // Security group allow outbound, deny inbound
  const securityGroup = new aws.ec2.SecurityGroup(`${serviceName}-security-group`, {
    name: serviceName,
    vpcId,
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

  // EFS filesystem for persisting external prover top-up state across Fargate runs.
  // Always provisioned so that enabling/disabling external top-ups is a pure runtime
  // flag change (--enable-external-topup) without redeploying infra.
  const efsFileSystem = new aws.efs.FileSystem(`${serviceName}-efs`, {
    encrypted: true,
    tags: { Name: `${serviceName}-topup-state` },
  });

  const efsMountTarget = privateSubnetIds.apply((subnets: string[]) =>
    subnets.map((subnetId, i) =>
      new aws.efs.MountTarget(`${serviceName}-efs-mt-${i}`, {
        fileSystemId: efsFileSystem.id,
        subnetId,
        securityGroups: [securityGroup.id],
      })
    )
  );

  const efsAccessPoint = new aws.efs.AccessPoint(`${serviceName}-efs-ap`, {
    fileSystemId: efsFileSystem.id,
    posixUser: { uid: 1000, gid: 1000 },
    rootDirectory: {
      path: '/topup-state',
      creationInfo: { ownerUid: 1000, ownerGid: 1000, permissions: '755' },
    },
  });

  // Create an execution role that has permissions to access the necessary secrets
  const execRole = new aws.iam.Role(`${serviceName}-exec`, {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
      Service: 'ecs-tasks.amazonaws.com',
    }),
    managedPolicyArns: [aws.iam.ManagedPolicy.AmazonECSTaskExecutionRolePolicy],
  });

  const execRolePolicy = new aws.iam.RolePolicy(`${serviceName}-exec`, {
    role: execRole.id,
    policy: {
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
          Resource: [distributorPrivateKeySecret.arn, rpcUrlSecret.arn, slasherKeySecret.arn, proverKeysSecret.arn, orderGeneratorKeysSecret.arn],
        },
      ],
    },
  });

  const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, { name: serviceName });

  let distributorArgs = [
    ethThreshold ? `--eth-threshold ${ethThreshold}` : '',
    stakeThreshold ? `--stake-threshold ${stakeThreshold}` : '',
    ethTopUpAmount ? `--eth-top-up-amount ${ethTopUpAmount}` : '',
    stakeTopUpAmount ? `--stake-top-up-amount ${stakeTopUpAmount}` : '',
    proverEthDonateThreshold ? `--prover-eth-donate-threshold ${proverEthDonateThreshold}` : '',
    proverStakeDonateThreshold ? `--prover-stake-donate-threshold ${proverStakeDonateThreshold}` : '',
    distributorEthAlertThreshold ? `--distributor-eth-alert-threshold ${distributorEthAlertThreshold}` : '',
    distributorStakeAlertThreshold ? `--distributor-stake-alert-threshold ${distributorStakeAlertThreshold}` : '',
    offchainRequestorAddresses ? `--offchain-requestor-addresses ${offchainRequestorAddresses}` : '',
    enableExternalTopup ? `--enable-external-topup` : '',
    minBcyclesThreshold ? `--min-bcycles-threshold ${minBcyclesThreshold}` : '',
    externalCollateralThreshold ? `--external-collateral-threshold ${externalCollateralThreshold}` : '',
    externalPerTopUpAmount ? `--external-per-top-up-amount ${externalPerTopUpAmount}` : '',
    externalLifetimeAllowance ? `--external-lifetime-allowance ${externalLifetimeAllowance}` : '',
    chainalysisOracleAddress ? `--chainalysis-oracle-address ${chainalysisOracleAddress}` : '',
    `--allowance-state-file /mnt/topup-state/topup-state.json`,
  ]

  if (boundlessMarketAddr && setVerifierAddr && collateralTokenAddr) {
    distributorArgs.push(
      `--boundless-market-address ${boundlessMarketAddr}`,
      `--set-verifier-address ${setVerifierAddr}`,
      `--market-chain-id ${chainId}`,
      `--collateral-token-address ${collateralTokenAddr}`,
    );
  } else if (boundlessMarketAddr || setVerifierAddr || collateralTokenAddr) {
    throw new Error('Must provide all of boundlessMarketAddr, setVerifierAddr, and collateralTokenAddr. Or none of them.');
  }

  const distributorSecrets = [
    {
      name: 'RPC_URL',
      valueFrom: rpcUrlSecret.arn,
    },
    {
      name: 'PRIVATE_KEY',
      valueFrom: distributorPrivateKeySecret.arn,
    },
    {
      name: 'SLASHER_KEY',
      valueFrom: slasherKeySecret.arn,
    },
    {
      name: 'PROVER_KEYS',
      valueFrom: proverKeysSecret.arn,
    },
    {
      name: 'ORDER_GENERATOR_KEYS',
      valueFrom: orderGeneratorKeysSecret.arn,
    },
    ...(indexerApiUrlSecret
      ? [{ name: 'INDEXER_API_URL', valueFrom: indexerApiUrlSecret.arn }]
      : []),
  ];

  // IAM Role for EventBridge to Start ECS Tasks and log failures
  const eventBridgeRole = new aws.iam.Role(`${serviceName}-event-bridge-role`, {
    assumeRolePolicy: JSON.stringify({
      Version: '2012-10-17',
      Statement: [
        {
          Action: 'sts:AssumeRole',
          Principal: { Service: 'events.amazonaws.com' },
          Effect: 'Allow',
        },
      ],
    }),
    managedPolicyArns: [
      aws.iam.ManagedPolicy.AmazonECSTaskExecutionRolePolicy,
      aws.iam.ManagedPolicy.AmazonEC2ContainerServiceEventsRole,
    ],
  });

  const rule = new aws.cloudwatch.EventRule(`${serviceName}-schedule-rule`, {
    scheduleExpression: `rate(${scheduleMinutes} minutes)`,
  });

  // EFS mount points and volumes — always attached so toggling external top-ups
  // is a runtime-only change via --enable-external-topup.
  const containerMountPoints = [{
    sourceVolume: 'topup-state',
    containerPath: '/mnt/topup-state',
    readOnly: false,
  }];

  const taskVolumes = [{
    name: 'topup-state',
    efsVolumeConfiguration: {
      fileSystemId: efsFileSystem.id,
      transitEncryption: 'ENABLED' as const,
      authorizationConfig: {
        accessPointId: efsAccessPoint.id,
        iam: 'ENABLED' as const,
      },
    },
  }];

  // Create an ECS Task Definition for Fargate
  const fargateTask = new awsx.ecs.FargateTaskDefinition(
    `${serviceName}-task`,
    {
      container: {
        name: serviceName,
        image: image.ref,
        cpu: 128,
        memory: 512,
        essential: true,
        entryPoint: ['/bin/sh', '-c'],
        command: [
          `/app/boundless-distributor ${distributorArgs.join(' ')}`,
        ],
        environment: [
          {
            name: 'RUST_LOG',
            value: logLevel,
          },
          {
            name: 'SECRET_HASH',
            value: secretHash,
          },
        ],
        secrets: distributorSecrets,
        mountPoints: containerMountPoints,
      },
      volumes: taskVolumes,
      logGroup: {
        args: { name: serviceName, retentionInDays: 0 },
      },
      executionRole: {
        roleArn: execRole.arn,
      },
    },
    { dependsOn: [execRole, execRolePolicy] }
  );

  // EventBridge Target to Start Task
  new aws.cloudwatch.EventTarget(`${serviceName}-task-target`, {
    rule: rule.name,
    arn: cluster.arn,
    roleArn: eventBridgeRole.arn,
    ecsTarget: {
      taskDefinitionArn: fargateTask.taskDefinition.arn,
      launchType: 'FARGATE',
      networkConfiguration: {
        securityGroups: [securityGroup.id],
        subnets: privateSubnetIds,
      },
    },
  });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-error-filter`, {
    name: `${serviceName}-log-err-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-err`,
      value: '1',
      defaultValue: '0',
    },
    pattern: 'ERROR',
  }, { dependsOn: [fargateTask] });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-stake-filter`, {
    name: `${serviceName}-log-stake-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-stake`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '"[B-DIST-STK]"',
  }, { dependsOn: [fargateTask] });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-eth-filter`, {
    name: `${serviceName}-log-eth-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-eth`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '"[B-DIST-ETH]"',
  }, { dependsOn: [fargateTask] });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-eth-low-filter`, {
    name: `${serviceName}-log-eth-low-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-eth-low`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '"[B-DIST-ETH-LOW]"',
  }, { dependsOn: [fargateTask] });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-stake-low-filter`, {
    name: `${serviceName}-log-stake-low-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-stake-low`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '"[B-DIST-STK-LOW]"',
  }, { dependsOn: [fargateTask] });

  // External prover top-up metric filters
  new aws.cloudwatch.LogMetricFilter(`${serviceName}-topup-ofac-filter`, {
    name: `${serviceName}-log-topup-ofac-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-topup-ofac`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '"[B-TOPUP-OFAC]"',
  }, { dependsOn: [fargateTask] });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-topup-ofac-err-filter`, {
    name: `${serviceName}-log-topup-ofac-err-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-topup-ofac-err`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '"[B-TOPUP-OFAC-ERR]"',
  }, { dependsOn: [fargateTask] });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-topup-ok-filter`, {
    name: `${serviceName}-log-topup-ok-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-topup-ok`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '"[B-TOPUP-OK]"',
  }, { dependsOn: [fargateTask] });

  const alarmActions = alertsTopicArns;

  new aws.cloudwatch.MetricAlarm(`${serviceName}-error-alarm-${Severity.SEV2}`, {
    name: `${serviceName}-log-err-${Severity.SEV2}`,
    metricQueries: [
      {
        id: 'm1',
        metric: {
          namespace: `Boundless/Services/${serviceName}`,
          metricName: `${serviceName}-log-err`,
          period: 60,
          stat: 'Sum',
        },
        returnData: true,
      },
    ],
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    // >=2 error periods per hour
    evaluationPeriods: 60,
    datapointsToAlarm: chainId === '11155111' ? 10 : 2, // Sepolia is flakey and has issues with tx timeouts
    treatMissingData: 'notBreaching',
    alarmDescription: 'Distributor log ERROR level 2 times in one hour',
    actionsEnabled: true,
    alarmActions,
  });

  new aws.cloudwatch.MetricAlarm(`${serviceName}-stake-alarm-${Severity.SEV2}`, {
    name: `${serviceName}-log-stake-${Severity.SEV2}`,
    metricQueries: [
      {
        id: 'm1',
        metric: {
          namespace: `Boundless/Services/${serviceName}`,
          metricName: `${serviceName}-log-stake`,
          period: 3600,
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
    alarmDescription: `Distributor unable to send stake. Send stake to distributor: ${distributorAddress} on ${chainId}`,
    actionsEnabled: true,
    alarmActions,
  });

  new aws.cloudwatch.MetricAlarm(`${serviceName}-eth-alarm-${Severity.SEV1}`, {
    name: `${serviceName}-log-eth-${Severity.SEV1}`,
    metricQueries: [
      {
        id: 'm1',
        metric: {
          namespace: `Boundless/Services/${serviceName}`,
          metricName: `${serviceName}-log-eth`,
          period: 3600,
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
    alarmDescription: `Distributor unable to send ETH. Send ETH to distributor: ${distributorAddress} on ${chainId}`,
    actionsEnabled: true,
    alarmActions,
  });

  new aws.cloudwatch.MetricAlarm(`${serviceName}-stake-low-alarm-${Severity.SEV2}`, {
    name: `${serviceName}-log-stake-low-${Severity.SEV2}`,
    metricQueries: [
      {
        id: 'm1',
        metric: {
          namespace: `Boundless/Services/${serviceName}`,
          metricName: `${serviceName}-log-stake-low`,
          period: 3600,
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
    alarmDescription: `Distributor stake balance low. Send stake to distributor: ${distributorAddress} on ${chainId}`,
    actionsEnabled: true,
    alarmActions,
  });

  new aws.cloudwatch.MetricAlarm(`${serviceName}-eth-low-alarm-${Severity.SEV2}`, {
    name: `${serviceName}-log-eth-low-${Severity.SEV2}`,
    metricQueries: [
      {
        id: 'm1',
        metric: {
          namespace: `Boundless/Services/${serviceName}`,
          metricName: `${serviceName}-log-eth-low`,
          period: 3600,
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
    alarmDescription: `WARNING: Distributor ETH balance low. Send ETH to distributor: ${distributorAddress} on ${chainId}`,
    actionsEnabled: true,
    alarmActions,
  });

};
