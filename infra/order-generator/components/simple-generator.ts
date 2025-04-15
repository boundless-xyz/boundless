import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import * as docker_build from '@pulumi/docker-build';
import { ChainId, getServiceNameV1 } from '../../util';

interface SimpleGeneratorArgs {
  chainId: string;
  stackName: string;
  privateKey: pulumi.Output<string>;
  pinataJWT: pulumi.Output<string>;
  ethRpcUrl: pulumi.Output<string>;
  orderStreamUrl?: pulumi.Output<string>;
  githubTokenSecret?: pulumi.Output<string>;
  logLevel: string;
  dockerDir: string;
  dockerTag: string;
  setVerifierAddr: string;
  boundlessMarketAddr: string;
  pinataGateway: string;
  interval: string;
  lockStake: string;
  rampUp: string;
  minPricePerMCycle: string;
  maxPricePerMCycle: string;
  vpcId: pulumi.Output<string>;
  privateSubnetIds: pulumi.Output<string[]>;
  boundlessAlertsTopicArn?: string;
}

export class SimpleGenerator extends pulumi.ComponentResource {
  constructor(name: string, args: SimpleGeneratorArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:order-generator:${name}`, name, args, opts);

    const serviceName = getServiceNameV1(args.stackName, "order-generator", args.chainId);

    const privateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-private-key`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-private-key-v1`, {
      secretId: privateKeySecret.id,
      secretString: args.privateKey,
    });

    const pinataJwtSecret = new aws.secretsmanager.Secret(`${serviceName}-pinata-jwt`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-pinata-jwt-v1`, {
      secretId: pinataJwtSecret.id,
      secretString: args.pinataJWT,
    });

    const rpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-rpc-url`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-rpc-url`, {
      secretId: rpcUrlSecret.id,
      secretString: args.ethRpcUrl,
    });

    const orderStreamUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-order-stream-url`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-order-stream-url`, {
      secretId: orderStreamUrlSecret.id,
      secretString: args.orderStreamUrl,
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

    let buildSecrets = {};
    if (args.githubTokenSecret !== undefined) {
      buildSecrets = {
        ...buildSecrets,
        githubTokenSecret: args.githubTokenSecret
      }
    }

    const dockerTagPath = pulumi.interpolate`${repo.repository.repositoryUrl}:${args.dockerTag}`;

    const image = new docker_build.Image(`${serviceName}-image`, {
      tags: [dockerTagPath],
      context: {
        location: args.dockerDir,
      },
      platforms: ['linux/amd64'],
      push: true,
      dockerfile: {
        location: `${args.dockerDir}/dockerfiles/order_generator.dockerfile`,
      },
      secrets: buildSecrets,
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

    const securityGroup = new aws.ec2.SecurityGroup(`${serviceName}-security-group`, {
      name: serviceName,
      vpcId: args.vpcId,
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
            Resource: [privateKeySecret.arn, pinataJwtSecret.arn, rpcUrlSecret.arn, orderStreamUrlSecret.arn],
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
          subnets: args.privateSubnetIds,
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
              `/app/boundless-order-generator --interval ${args.interval} --min ${args.minPricePerMCycle} --max ${args.maxPricePerMCycle} --lockin-stake ${args.lockStake} --ramp-up ${args.rampUp} --set-verifier-address ${args.setVerifierAddr} --boundless-market-address ${args.boundlessMarketAddr}`,
            ],
            environment: [
              {
                name: 'IPFS_GATEWAY_URL',
                value: args.pinataGateway,
              },
              {
                name: 'RUST_LOG',
                value: args.logLevel,
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
              {
                name: 'ORDER_STREAM_URL',
                valueFrom: orderStreamUrlSecret.arn,
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

    const alarmActions = args.boundlessAlertsTopicArn ? [args.boundlessAlertsTopicArn] : [];

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
      evaluationPeriods: 60,
      datapointsToAlarm: 2,
      treatMissingData: 'notBreaching',
      alarmDescription: 'Order generator log ERROR level',
      actionsEnabled: true,
      alarmActions,
    });
  }
}
