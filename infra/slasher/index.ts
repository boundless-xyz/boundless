import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import * as docker_build from '@pulumi/docker-build';

const getEnvVar = (name: string) => {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Environment variable ${name} is not set`);
  }
  return value;
};

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const prefix = isDev ? `${getEnvVar("DEV_NAME")}-` : `${stackName}-`;
  const serviceName = `${prefix}order-slasher`;
  
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

  const image = new awsx.ecr.Image(`${serviceName}-image`, {
    repositoryUrl: repo.url,
    context: dockerDir,
    dockerfile: `${dockerDir}/dockerfiles/slasher.dockerfile`,
    imageTag: dockerTag,
    platform: 'linux/amd64',
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
  new awsx.ecs.FargateService(
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
          image: image.imageUri,
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
};
