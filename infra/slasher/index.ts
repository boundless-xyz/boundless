import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import * as docker_build from '@pulumi/docker-build';
import { ChainId, getServiceNameV1, getEnvVar, Severity } from '../util';
import * as crypto from 'crypto';
require('dotenv').config();

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName.includes("dev");
  const chainId = isDev ? getEnvVar("CHAIN_ID") : config.require('CHAIN_ID');
  const serviceName = getServiceNameV1(stackName, "order-slasher", chainId);

  const privateKey = isDev ? getEnvVar("PRIVATE_KEY") : config.requireSecret('PRIVATE_KEY');
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;
  const boundlessMarketAddr = isDev ? getEnvVar("BOUNDLESS_MARKET_ADDR") : config.require('BOUNDLESS_MARKET_ADDR');

  const logLevel = config.get('LOG_LEVEL') || 'info';
  const dockerDir = config.get('DOCKER_DIR') || '../../';
  const dockerTag = config.get('DOCKER_TAG') || 'latest';

  const githubTokenSecret = config.get('GH_TOKEN_SECRET');
  const interval = config.get('INTERVAL') || "60";
  const retries = config.get('RETRIES') || "3";
  const skipAddresses = config.get('SKIP_ADDRESSES');
  const maxBlockRange = config.get('MAX_BLOCK_RANGE');
  const startBlock = config.get('START_BLOCK');

  const baseStackName = isDev ? getEnvVar("BASE_STACK") : config.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');
  const txTimeout = config.get('TX_TIMEOUT') || "60";
  const warnBalanceBelow = config.get('WARN_BALANCE_BELOW');
  const errorBalanceBelow = config.get('ERROR_BALANCE_BELOW');

  const rdsPassword = isDev ? getEnvVar("RDS_PASSWORD") : config.requireSecret('RDS_PASSWORD');

  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');
  const boundlessPagerdutyTopicArn = config.get('PAGERDUTY_ALERTS_TOPIC_ARN');
  const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];

  const privateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-private-key`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-private-key-v1`, {
    secretId: privateKeySecret.id,
    secretString: privateKey,
  });

  const rpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-rpc-url`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-rpc-url`, {
    secretId: rpcUrlSecret.id,
    secretString: ethRpcUrl,
  });

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
  if (githubTokenSecret !== undefined) {
    buildSecrets = {
      githubTokenSecret
    }
  }

  const image = new docker_build.Image(`${serviceName}-image`, {
    tags: [dockerTagPath],
    context: {
      location: dockerDir,
    },
    // Due to limitations with cargo-chef, we need to build for amd64, even though slasher doesn't
    // strictly need r0vm. See `dockerfiles/slasher.dockerfile` for more details.
    platforms: ['linux/amd64'],
    secrets: buildSecrets,
    push: true,
    builder: dockerRemoteBuilder ? {
      name: dockerRemoteBuilder,
    } : undefined,
    dockerfile: {
      location: `${dockerDir}/dockerfiles/slasher.dockerfile`,
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

  // RDS PostgreSQL setup
  const dbSubnets = new aws.rds.SubnetGroup(`${serviceName}-dbsubnets`, {
    subnetIds: privateSubnetIds,
  });

  const rdsPort = 5432;
  const rdsSecurityGroup = new aws.ec2.SecurityGroup(`${serviceName}-rds`, {
    name: `${serviceName}-rds`,
    vpcId,
    ingress: [
      {
        fromPort: rdsPort,
        toPort: rdsPort,
        protocol: 'tcp',
        securityGroups: [securityGroup.id],
      },
    ],
    egress: [
      {
        fromPort: 0,
        toPort: 0,
        protocol: '-1',
        cidrBlocks: ['0.0.0.0/0'],
      },
    ],
  });

  const rdsUser = 'slasher';
  const rdsDbName = 'slasher';

  const rdsInstance = new aws.rds.Instance(`${serviceName}-postgres`, {
    identifier: `${serviceName}-postgres`,
    engine: 'postgres',
    engineVersion: '17.4',
    instanceClass: 'db.t4g.micro',
    allocatedStorage: 20,
    maxAllocatedStorage: 100,
    storageType: 'gp3',
    storageEncrypted: true,
    dbName: rdsDbName,
    username: rdsUser,
    password: rdsPassword,
    port: rdsPort,
    publiclyAccessible: false,
    dbSubnetGroupName: dbSubnets.name,
    vpcSecurityGroupIds: [rdsSecurityGroup.id],
    skipFinalSnapshot: true,
    backupRetentionPeriod: 7,
  });

  const dbUrlSecretValue = pulumi.interpolate`postgres://${rdsUser}:${rdsPassword}@${rdsInstance.address}:${rdsPort}/${rdsDbName}?sslmode=require`;
  const dbUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-db-url`);
  const dbUrlSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-db-url-v1`, {
    secretId: dbUrlSecret.id,
    secretString: dbUrlSecretValue,
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
          Resource: [privateKeySecret.arn, rpcUrlSecret.arn, dbUrlSecret.arn],
        },
      ],
    },
  });

  const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, { name: serviceName });

  let slasherArgs = [
    `--tx-timeout ${txTimeout}`,
    `--interval ${interval}`,
    `--retries ${retries}`,
  ]
  if (skipAddresses) {
    slasherArgs.push(`--skip-addresses ${skipAddresses}`);
  }
  if (warnBalanceBelow) {
    slasherArgs.push(`--warn-balance-below ${warnBalanceBelow}`);
  }
  if (errorBalanceBelow) {
    slasherArgs.push(`--error-balance-below ${errorBalanceBelow}`);
  }
  if (maxBlockRange) {
    slasherArgs.push(`--max-block-range ${maxBlockRange}`);
  }
  if (startBlock) {
    slasherArgs.push(`--start-block ${startBlock}`);
  }

  const service = new awsx.ecs.FargateService(
    `${serviceName}-service`,
    {
      name: serviceName,
      cluster: cluster.arn,
      networkConfiguration: {
        securityGroups: [securityGroup.id],
        subnets: privateSubnetIds,
      },
      taskDefinitionArgs: {
        logGroup: {
          args: { name: serviceName, retentionInDays: 0 },
        },
        executionRole: {
          roleArn: execRole.arn,
        },
        container: {
          name: serviceName,
          image: image.ref,
          cpu: 128,
          memory: 512,
          essential: true,
          entryPoint: ['/bin/sh', '-c'],
          command: [
            `/app/boundless-slasher ${slasherArgs.join(' ')}`,
          ],
          environment: [
            {
              name: 'RUST_LOG',
              value: logLevel,
            },
            {
              name: 'BOUNDLESS_MARKET_ADDRESS',
              value: boundlessMarketAddr,
            },
            {
              name: 'SECRET_HASH',
              value: secretHash,
            },
          ],
          secrets: [
            {
              name: 'RPC_URL',
              valueFrom: rpcUrlSecret.arn,
            },
            {
              name: 'PRIVATE_KEY',
              valueFrom: privateKeySecret.arn,
            },
            {
              name: 'DB_URL',
              valueFrom: dbUrlSecret.arn,
            },
          ],
        },
      },
    },
    { dependsOn: [execRole, execRolePolicy, rdsInstance, dbUrlSecretVersion] }
  );

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
  }, { dependsOn: [service] });

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
    alarmDescription: 'Order slasher log ERROR level 2 times in one hour',
    actionsEnabled: true,
    alarmActions,
  });

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-fatal-filter`, {
    name: `${serviceName}-log-fatal-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-fatal`,
      value: '1',
      defaultValue: '0',
    },
    pattern: 'FATAL',
  }, { dependsOn: [service] });

  new aws.cloudwatch.MetricAlarm(`${serviceName}-fatal-alarm-${Severity.SEV2}`, {
    name: `${serviceName}-log-fatal-${Severity.SEV2}`,
    metricQueries: [
      {
        id: 'm1',
        metric: {
          namespace: `Boundless/Services/${serviceName}`,
          metricName: `${serviceName}-log-fatal`,
          period: 60,
          stat: 'Sum',
        },
        returnData: true,
      },
    ],
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    evaluationPeriods: 60,
    datapointsToAlarm: chainId === '11155111' ? 10 : 2, // Sepolia is flakey and has issues with tx timeouts
    treatMissingData: 'notBreaching',
    alarmDescription: `Order slasher FATAL (task exited) twice in 1 hour`,
    actionsEnabled: true,
    alarmActions,
  });
};
