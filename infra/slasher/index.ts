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
  const isDev = stackName === "dev";
  const chainId = config.require('CHAIN_ID');
  const serviceName = getServiceNameV1(stackName, "order-slasher", chainId);

  const privateKey = isDev ? getEnvVar("PRIVATE_KEY") : config.requireSecret('PRIVATE_KEY');
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;

  const logLevel = config.require('LOG_LEVEL');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');
  const boundlessMarketAddr = config.require('BOUNDLESS_MARKET_ADDR');

  const githubTokenSecret = config.get('GH_TOKEN_SECRET');
  const interval = config.require('INTERVAL');
  const retries = config.require('RETRIES');
  const skipAddresses = config.get('SKIP_ADDRESSES');

  const baseStackName = config.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');
  const txTimeout = config.require('TX_TIMEOUT');

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
    .apply(([_ethRpcUrl, _privateKey]) => {
      const hash = crypto.createHash("sha1");
      hash.update(_ethRpcUrl);
      hash.update(_privateKey);
      return hash.digest("hex");
    });

  const repo = new awsx.ecr.Repository(`${serviceName}-repo`, {
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

  new aws.ec2.SecurityGroupRule(`${serviceName}-efs-inbound`, {
    type: 'ingress',
    fromPort: 2049,
    toPort: 2049,
    protocol: 'tcp',
    securityGroupId: securityGroup.id,
    sourceSecurityGroupId: securityGroup.id,
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
          Resource: [privateKeySecret.arn, rpcUrlSecret.arn],
        },
      ],
    },
  });

  // EFS
  const fileSystem = new aws.efs.FileSystem(`${serviceName}-efs-rev4`, {
    encrypted: true,
    tags: {
      Name: serviceName,
    },
  });

  const mountTargets = privateSubnetIds.apply((subnets) =>
    subnets.map((subnetId: string, index: number) => {
      return new aws.efs.MountTarget(`${serviceName}-mount-${index}`, {
        fileSystemId: fileSystem.id,
        subnetId: subnetId,
        securityGroups: [securityGroup.id],
      }, { dependsOn: [fileSystem] });
    })
  );

  const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, { name: serviceName });
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
        volumes: [
          {
            name: 'slasher-storage',
            efsVolumeConfiguration: {
              fileSystemId: fileSystem.id,
              rootDirectory: '/',
            },
          },
        ],
        container: {
          name: serviceName,
          image: image.ref,
          cpu: 128,
          memory: 512,
          essential: true,
          mountPoints: [
            {
              sourceVolume: 'slasher-storage',
              containerPath: '/app/data',
              readOnly: false,
            },
          ],
          entryPoint: ['/bin/sh', '-c'],
          command: [
            `/app/boundless-slasher --db sqlite:///app/data/slasher.db --tx-timeout ${txTimeout} --interval ${interval} --retries ${retries} ${skipAddresses ? `--skip-addresses ${skipAddresses}` : ''}`,
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
          ],
        },
      },
    },
    { dependsOn: [execRole, execRolePolicy, mountTargets, fileSystem] }
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
    datapointsToAlarm: 2,
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
    datapointsToAlarm: 2,
    treatMissingData: 'notBreaching',
    alarmDescription: `Order slasher FATAL (task exited) twice in 1 hour`,
    actionsEnabled: true,
    alarmActions,
  });
};
