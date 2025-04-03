import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';

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
  const serviceName = `${prefix}order-generator`;

  const privateKey = isDev ? getEnvVar("PRIVATE_KEY") : config.requireSecret('PRIVATE_KEY');
  const pinataJWT = isDev ? getEnvVar("PINATA_JWT") : config.requireSecret('PINATA_JWT');
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');

  const logLevel = config.require('LOG_LEVEL');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');
  const setVerifierAddr = config.require('SET_VERIFIER_ADDR');
  const boundlessMarketAddr = config.require('BOUNDLESS_MARKET_ADDR');
  const pinataGateway = config.require('PINATA_GATEWAY_URL');
  const orderStreamUrl = config.require('ORDER_STREAM_URL');
  const interval = config.require('INTERVAL');
  const lockStake = config.require('LOCK_STAKE');
  const rampUp = config.require('RAMP_UP');
  const minPricePerMCycle = config.require('MIN_PRICE_PER_MCYCLE');
  const maxPricePerMCycle = config.require('MAX_PRICE_PER_MCYCLE');
  const baseStackName = config.require('BASE_STACK');
  
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');

  const privateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-private-key`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-private-key-v1`, {
    secretId: privateKeySecret.id,
    secretString: privateKey,
  });

  const pinataJwtSecret = new aws.secretsmanager.Secret(`${serviceName}-pinata-jwt`);
  new aws.secretsmanager.SecretVersion(`${serviceName}-pinata-jwt-v1`, {
    secretId: pinataJwtSecret.id,
    secretString: pinataJWT,
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
    dockerfile: `${dockerDir}/dockerfiles/order_generator.dockerfile`,
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
  const execRole = new aws.iam.Role(`${serviceName}-exec-1`, {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
      Service: 'ecs-tasks.amazonaws.com',
    }),
    managedPolicyArns: [aws.iam.ManagedPolicy.AmazonECSTaskExecutionRolePolicy],
  });

  const execRolePolicy = new aws.iam.RolePolicy(`${serviceName}-exec-1`, {
    role: execRole.id,
    policy: {
      Version: '2012-10-17',
      Statement: [
        {
          Effect: 'Allow',
          Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
          Resource: [privateKeySecret.arn, pinataJwtSecret.arn, rpcUrlSecret.arn],
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
            `/app/boundless-order-generator --interval ${interval} --min ${minPricePerMCycle} --max ${maxPricePerMCycle} --lockin-stake ${lockStake} --ramp-up ${rampUp} --set-verifier-address ${setVerifierAddr} --boundless-market-address ${boundlessMarketAddr} --order-stream-url ${orderStreamUrl}`,
          ],
          environment: [
            {
              name: 'PINATA_GATEWAY_URL',
              value: pinataGateway,
            },
            {
              name: 'RUST_LOG',
              value: logLevel,
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
              name: 'PINATA_JWT',
              valueFrom: pinataJwtSecret.arn,
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
  });
};
