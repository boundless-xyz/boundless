import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import * as docker_build from '@pulumi/docker-build';
import { ChainId, getServiceName, getEnvVar } from '../util';

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const serviceName = getServiceName(stackName, "order-slasher", ChainId.SEPOLIA);
  
  const privateKey = isDev ? getEnvVar("PRIVATE_KEY") : config.requireSecret('PRIVATE_KEY');
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');

  const logLevel = config.require('LOG_LEVEL');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');
  const boundlessMarketAddr = config.require('BOUNDLESS_MARKET_ADDR');
  
  const interval = config.require('INTERVAL');
  const retries = config.require('RETRIES');
  const skipAddresses = config.require('SKIP_ADDRESSES');

  const baseStackName = config.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');

  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');

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

  const image = new docker_build.Image(`${serviceName}-image`, {
    tags: [dockerTagPath],
    context: {
      location: dockerDir,
    },
    platforms: ['linux/amd64'],
    push: true,
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
        container: {
          name: serviceName,
          image: image.ref,
          cpu: 128,
          memory: 512,
          essential: true,
          entryPoint: ['/bin/sh', '-c'],
          command: [
            `/app/boundless-slasher --interval ${interval} --retries ${retries} --skip-addresses ${skipAddresses}`,
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
    { dependsOn: [execRole, execRolePolicy] }
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
    pattern: '?ERROR ?error ?Error',
  }, { dependsOn: [service] });

  const alarmActions = boundlessAlertsTopicArn ? [boundlessAlertsTopicArn] : [];

  new aws.cloudwatch.MetricAlarm(`${serviceName}-error-alarm`, {
    name: `${serviceName}-log-err`,
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
    period: 60,
    datapointsToAlarm: 2,
    treatMissingData: 'notBreaching',
    alarmDescription: 'Order generator log ERROR level',
    actionsEnabled: true,
    alarmActions,
  });
};
