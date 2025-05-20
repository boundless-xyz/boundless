import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import { Image } from '@pulumi/docker-build';
import { getServiceNameV1, Severity } from '../../util';
import * as crypto from 'crypto';

interface OrderGeneratorArgs {
  chainId: string;
  stackName: string;
  privateKey: pulumi.Output<string>;
  pinataJWT: pulumi.Output<string>;
  ethRpcUrl: pulumi.Output<string>;
  image: Image;
  logLevel: string;
  setVerifierAddr: string;
  boundlessMarketAddr: string;
  pinataGateway: string;
  interval: string;
  lockStake: string;
  rampUp: string;
  minPricePerMCycle: string;
  maxPricePerMCycle: string;
  secondsPerMCycle: string;
  vpcId: pulumi.Output<string>;
  privateSubnetIds: pulumi.Output<string[]>;
  boundlessAlertsTopicArns?: string[];
  offchainConfig?: {
    autoDeposit: string;
    orderStreamUrl: pulumi.Output<string>;
  };
  txTimeout: string;
}

export class OrderGenerator extends pulumi.ComponentResource {
  constructor(name: string, args: OrderGeneratorArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:order-generator:${name}`, name, args, opts);

    const serviceName = getServiceNameV1(args.stackName, `og-${name}`, args.chainId);

    const offchainConfig = args.offchainConfig;

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
      secretString: offchainConfig?.orderStreamUrl ?? 'none',
    });

    const secretHash = pulumi
      .all([args.ethRpcUrl, args.privateKey, offchainConfig?.orderStreamUrl])
      .apply(([_ethRpcUrl, _privateKey, _orderStreamUrl]) => {
        const hash = crypto.createHash("sha1");
        hash.update(_ethRpcUrl);
        hash.update(_privateKey);
        hash.update(_orderStreamUrl ?? '');
        return hash.digest("hex");
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

    let environment = [
      {
        name: 'IPFS_GATEWAY_URL',
        value: args.pinataGateway,
      },
      {
        name: 'RUST_LOG',
        value: args.logLevel,
      },
      { name: 'NO_COLOR', value: '1' },
      { name: 'SECRET_HASH', value: secretHash },
    ]

    let secrets = [
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
    ];

    if (offchainConfig) {
      environment.push({
        name: 'AUTO_DEPOSIT',
        value: offchainConfig.autoDeposit,
      });
      secrets.push({
        name: 'ORDER_STREAM_URL',
        valueFrom: orderStreamUrlSecret.arn,
      });
    };

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
            image: args.image.ref,
            cpu: 512,
            memory: 512,
            essential: true,
            entryPoint: ['/bin/sh', '-c'],
            command: [
              `/app/boundless-order-generator --interval ${args.interval} --min ${args.minPricePerMCycle} --max ${args.maxPricePerMCycle} --lockin-stake ${args.lockStake} --ramp-up ${args.rampUp} --set-verifier-address ${args.setVerifierAddr} --boundless-market-address ${args.boundlessMarketAddr} --seconds-per-mcycle ${args.secondsPerMCycle} --tx-timeout ${args.txTimeout}`,
            ],
            environment,
            secrets,
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

    const alarmActions = args.boundlessAlertsTopicArns ?? [];

    // 3 errors within 1 hour in the order generator triggers a SEV2 alarm.
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
      evaluationPeriods: 60,
      datapointsToAlarm: 3,
      treatMissingData: 'notBreaching',
      alarmDescription: `Order generator ${name} log ERROR level 3 times within an hour`,
      actionsEnabled: true,
      alarmActions,
    });

    // 7 errors within 1 hour in the order generator triggers a SEV1 alarm.
    new aws.cloudwatch.MetricAlarm(`${serviceName}-error-alarm-${Severity.SEV1}`, {
      name: `${serviceName}-log-err-${Severity.SEV1}`,
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
      datapointsToAlarm: 7,
      treatMissingData: 'notBreaching',
      alarmDescription: `Order generator ${name} log ERROR level 7 times within an hour`,
      actionsEnabled: true,
      alarmActions,
    });

    // A single error in the order generator causes the process to exit.
    // SEV2 alarm if we see 2 errors in 30 mins.
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
      evaluationPeriods: 30,
      datapointsToAlarm: 2,
      treatMissingData: 'notBreaching',
      alarmDescription: `Order generator ${name} FATAL (task exited) 2 times within 30 mins`,
      actionsEnabled: true,
      alarmActions,
    });

    // A single error in the order generator causes the process to exit.
    // SEV1 alarm if we see 4 errors in 30 mins.
    new aws.cloudwatch.MetricAlarm(`${serviceName}-fatal-alarm-${Severity.SEV1}`, {
      name: `${serviceName}-log-fatal-${Severity.SEV1}`,
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
      evaluationPeriods: 30,
      datapointsToAlarm: 4,
      treatMissingData: 'notBreaching',
      alarmDescription: `Order generator ${name} FATAL (task exited) 4 times within 30 mins`,
      actionsEnabled: true,
      alarmActions,
    });
  }
}
