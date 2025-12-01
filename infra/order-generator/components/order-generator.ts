import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import { Image } from '@pulumi/docker-build';
import { getServiceNameV1, Severity } from '../../util';
import * as crypto from 'crypto';

const FARGATE_CPU = 512;
const FARGATE_MEMORY = 512;

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
  collateralTokenAddress?: string;
  ipfsGateway: string;
  interval: string;
  lockCollateralRaw: string;
  rampUp?: string;
  minPricePerMCycle: string;
  maxPricePerMCycle: string;
  secondsPerMCycle?: string;
  rampUpSecondsPerMCycle?: string;
  inputMaxMCycles?: string;
  vpcId: pulumi.Output<string>;
  privateSubnetIds: pulumi.Output<string[]>;
  boundlessAlertsTopicArns?: string[];
  offchainConfig?: {
    orderStreamUrl: pulumi.Output<string>;
  };
  autoDeposit?: string;
  warnBalanceBelow?: string;
  errorBalanceBelow?: string;
  txTimeout: string;
  lockTimeout?: string;
  timeout?: string;
  execRateKhz?: string;
  oneShotConfig?: {
    // Number of requests to submit.
    count: string;
    // Schedule expression for the event bridge rule.
    scheduleExpression: string;
  };
}

export class OrderGenerator extends pulumi.ComponentResource {
  constructor(name: string, args: OrderGeneratorArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:order-generator:${name}`, name, args, opts);

    const serviceName = getServiceNameV1(args.stackName, `og-${name}`, args.chainId);
    const isStaging = args.stackName.includes('staging');

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
      .apply(([_ethRpcUrl, _privateKey, _orderStreamUrl]: [string, string, string | undefined]) => {
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
        value: args.ipfsGateway,
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

    if (args.autoDeposit) {
      environment.push({
        name: 'AUTO_DEPOSIT',
        value: args.autoDeposit,
      });
    }

    if (offchainConfig) {
      secrets.push({
        name: 'ORDER_STREAM_URL',
        valueFrom: orderStreamUrlSecret.arn,
      });
    };

    let ogArgs = [
      `--interval ${args.interval}`,
      `--min ${args.minPricePerMCycle}`,
      `--max ${args.maxPricePerMCycle}`,
      `--lock-collateral-raw ${args.lockCollateralRaw}`,
      `--set-verifier-address ${args.setVerifierAddr}`,
      `--boundless-market-address ${args.boundlessMarketAddr}`,
      `--tx-timeout ${args.txTimeout}`
    ]
    if (args.collateralTokenAddress) {
      ogArgs.push(`--collateral-token-address ${args.collateralTokenAddress}`);
    }
    if (offchainConfig) {
      ogArgs.push('--submit-offchain');
    }
    if (args.warnBalanceBelow) {
      ogArgs.push(`--warn-balance-below ${args.warnBalanceBelow}`);
    }
    if (args.errorBalanceBelow) {
      ogArgs.push(`--error-balance-below ${args.errorBalanceBelow}`);
    }
    if (args.inputMaxMCycles) {
      ogArgs.push(`--input-max-mcycles ${args.inputMaxMCycles}`);
    }
    if (args.lockTimeout) {
      ogArgs.push(`--lock-timeout ${args.lockTimeout}`);
    }
    if (args.timeout) {
      ogArgs.push(`--timeout ${args.timeout}`);
    }
    if (args.rampUp) {
      ogArgs.push(`--ramp-up ${args.rampUp}`);
    }
    if (args.rampUpSecondsPerMCycle) {
      ogArgs.push(`--ramp-up-seconds-per-mcycle ${args.rampUpSecondsPerMCycle}`);
    }
    if (args.secondsPerMCycle) {
      ogArgs.push(`--seconds-per-mcycle ${args.secondsPerMCycle}`);
    }
    if (args.execRateKhz) {
      ogArgs.push(`--exec-rate-khz ${args.execRateKhz}`);
    }
    if (args.oneShotConfig) {
      ogArgs.push(`--count ${args.oneShotConfig.count}`);
    }

    const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, { name: serviceName });

    // Create Fargate definition based on mode
    let logDependency: pulumi.Resource;
    if (args.oneShotConfig) {
      // Scheduled task mode: Create EventBridge rule + FargateTaskDefinition
      const eventBridgeRole = new aws.iam.Role(`${serviceName}-ev-br-role`, {
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
        scheduleExpression: args.oneShotConfig.scheduleExpression,
      });

      const fargateTask = new awsx.ecs.FargateTaskDefinition(
        `${serviceName}-task`,
        {
          container: {
            name: serviceName,
            image: args.image.ref,
            cpu: FARGATE_CPU,
            memory: FARGATE_MEMORY,
            essential: true,
            entryPoint: ['/bin/sh', '-c'],
            command: [
              `/app/boundless-order-generator ${ogArgs.join(' ')}`,
            ],
            environment,
            secrets,
          },
          logGroup: {
            args: { name: serviceName, retentionInDays: 0 },
          },
          executionRole: {
            roleArn: execRole.arn,
          },
        },
        { dependsOn: [execRole, execRolePolicy] }
      );

      new aws.cloudwatch.EventTarget(`${serviceName}-task-target`, {
        rule: rule.name,
        arn: cluster.arn,
        roleArn: eventBridgeRole.arn,
        ecsTarget: {
          taskDefinitionArn: fargateTask.taskDefinition.arn,
          launchType: 'FARGATE',
          networkConfiguration: {
            securityGroups: [securityGroup.id],
            subnets: args.privateSubnetIds,
          },
        },
      });

      logDependency = fargateTask;
    } else {
      // Service mode, runs indefinitely
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
              cpu: FARGATE_CPU,
              memory: FARGATE_MEMORY,
              essential: true,
              entryPoint: ['/bin/sh', '-c'],
              command: [
                `/app/boundless-order-generator ${ogArgs.join(' ')}`,
              ],
              environment,
              secrets,
            },
          },
        },
        { dependsOn: [execRole, execRolePolicy] }
      );
      logDependency = service;
    }

    // Common log metric filters and alarms for both modes
    // Exclude balance errors and transaction confirmation errors which have separate alarms.
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-error-filter`, {
      name: `${serviceName}-log-err-filter`,
      logGroupName: serviceName,
      metricTransformation: {
        namespace: `Boundless/Services/${serviceName}`,
        name: `${serviceName}-log-err`,
        value: '1',
        defaultValue: '0',
      },
      pattern: 'ERROR -"[B-BAL-ETH]" -"[B-OG-CONF]"',
    }, { dependsOn: [logDependency] });

    // Create a separate metric filter for transaction confirmation errors
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-tx-conf-error-filter`, {
      name: `${serviceName}-log-tx-conf-err-filter`,
      logGroupName: serviceName,
      metricTransformation: {
        namespace: `Boundless/Services/${serviceName}`,
        name: `${serviceName}-log-tx-conf-err`,
        value: '1',
        defaultValue: '0',
      },
      pattern: '"[B-OG-CONF]"',
    }, { dependsOn: [logDependency] });

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
    }, { dependsOn: [logDependency] });

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-bal-eth-filter-${Severity.SEV2}`, {
      name: `${serviceName}-log-bal-eth-filter-${Severity.SEV2}`,
      logGroupName: serviceName,
      metricTransformation: {
        namespace: `Boundless/Services/${serviceName}`,
        name: `${serviceName}-log-bal-eth-${Severity.SEV2}`,
        value: '1',
        defaultValue: '0',
      },
      pattern: 'WARN "[B-BAL-ETH]"',
    }, { dependsOn: [logDependency] });

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-bal-eth-filter-${Severity.SEV1}`, {
      name: `${serviceName}-log-bal-eth-filter-${Severity.SEV1}`,
      logGroupName: serviceName,
      metricTransformation: {
        namespace: `Boundless/Services/${serviceName}`,
        name: `${serviceName}-log-bal-eth-${Severity.SEV1}`,
        value: '1',
        defaultValue: '0',
      },
      pattern: 'ERROR "[B-BAL-ETH]"',
    }, { dependsOn: [logDependency] });

    const alarmActions = args.boundlessAlertsTopicArns ?? [];

    new aws.cloudwatch.MetricAlarm(`${serviceName}-low-bal-eth-alarm-${Severity.SEV2}`, {
      name: `${serviceName}-low-bal-eth-${Severity.SEV2}`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: `Boundless/Services/${serviceName}`,
            metricName: `${serviceName}-log-bal-eth-${Severity.SEV2}`,
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
      alarmDescription: `${name} ETH bal < ${args.warnBalanceBelow} ${Severity.SEV2}`,
      actionsEnabled: true,
      alarmActions,
    });

    if (!isStaging) {
      new aws.cloudwatch.MetricAlarm(`${serviceName}-low-bal-eth-alarm-${Severity.SEV1}`, {
        name: `${serviceName}-low-bal-eth-${Severity.SEV1}`,
        metricQueries: [
          {
            id: 'm1',
            metric: {
              namespace: `Boundless/Services/${serviceName}`,
              metricName: `${serviceName}-log-bal-eth-${Severity.SEV1}`,
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
        alarmDescription: `${name} ETH bal < ${args.errorBalanceBelow} ${Severity.SEV1}`,
        actionsEnabled: true,
        alarmActions,
      });
    }

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
      datapointsToAlarm: 4,
      treatMissingData: 'notBreaching',
      alarmDescription: `Order generator ${name} log ERROR level 3 times within an hour`,
      actionsEnabled: true,
      alarmActions,
    });

    // 7 errors within 1 hour in the order generator triggers a SEV1 alarm.
    // Eth Sepolia is unreliable, so don't SEV1 on it.
    if (!isStaging && args.chainId !== '11155111') {
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
    }

    // 10 or more transaction confirmation errors within 1 hour triggers a SEV2 alarm.
    new aws.cloudwatch.MetricAlarm(`${serviceName}-tx-conf-error-alarm-${Severity.SEV2}`, {
      name: `${serviceName}-log-tx-conf-err-${Severity.SEV2}`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: `Boundless/Services/${serviceName}`,
            metricName: `${serviceName}-log-tx-conf-err`,
            period: 3600,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 10,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 1,
      datapointsToAlarm: 1,
      treatMissingData: 'notBreaching',
      alarmDescription: `Order generator ${name} transaction confirmation error 10+ times within an hour`,
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
    // Eth Sepolia is unreliable, so don't SEV1 on it.
    if (!isStaging && args.chainId !== '11155111') {
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
}
