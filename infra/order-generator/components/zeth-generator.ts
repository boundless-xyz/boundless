import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import * as docker_build from '@pulumi/docker-build';
import { getServiceNameV1 } from '../../util';

interface ZethGeneratorArgs {
  chainId: string;
  stackName: string;
  privateKey: pulumi.Output<string>;
  pinataJWT: pulumi.Output<string>;
  zethRpcUrl: pulumi.Output<string>;
  boundlessRpcUrl: pulumi.Output<string>;
  orderStreamUrl?: pulumi.Output<string>;
  githubTokenSecret?: pulumi.Output<string>;
  logLevel: string;
  dockerDir: string;
  dockerTag: string;
  dockerRemoteBuilder?: string;
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
  retries: string;
  scheduleMinutes: string;
  timeout: string;
  lockTimeout: string;
}

export class ZethGenerator extends pulumi.ComponentResource {
  constructor(name: string, args: ZethGeneratorArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:order-generator:${name}`, name, args, opts);

    const { 
      chainId, 
      stackName, 
      privateKey, 
      pinataJWT, 
      zethRpcUrl, 
      boundlessRpcUrl, 
      orderStreamUrl, 
      githubTokenSecret, 
      logLevel, 
      dockerDir, 
      dockerTag, 
      dockerRemoteBuilder,
      setVerifierAddr, 
      boundlessMarketAddr, 
      pinataGateway, 
      interval, 
      lockStake, 
      rampUp, 
      minPricePerMCycle,
      maxPricePerMCycle,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArn,
      retries,
      scheduleMinutes,
      timeout,
      lockTimeout,
    } = args;
    
    const serviceName = getServiceNameV1(args.stackName, "zeth-order-generator", args.chainId);

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

    const zethRpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-zeth-rpc-url`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-zeth-rpc-url`, {
      secretId: zethRpcUrlSecret.id,
      secretString: zethRpcUrl,
    });

    const boundlessRpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-boundless-rpc-url`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-boundless-rpc-url`, {
      secretId: boundlessRpcUrlSecret.id,
      secretString: boundlessRpcUrl,
    });

    const orderStreamUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-order-stream-url`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-order-stream-url`, {
      secretId: orderStreamUrlSecret.id,
      secretString: orderStreamUrl,
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
    if (githubTokenSecret !== undefined) {
      buildSecrets = {
        ...buildSecrets,
        githubTokenSecret: githubTokenSecret
      }
    }

    const dockerTagPath = pulumi.interpolate`${repo.repository.repositoryUrl}:${dockerTag}`;

    const image = new docker_build.Image(`${serviceName}-image`, {
      tags: [dockerTagPath],
      context: {
        location: dockerDir,
      },
      builder: dockerRemoteBuilder ? {
        name: dockerRemoteBuilder,
      } : undefined,
      platforms: ['linux/amd64'],
      push: true,
      dockerfile: {
        location: `${dockerDir}/dockerfiles/order_generator_zeth.dockerfile`,
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
            Resource: [privateKeySecret.arn, pinataJwtSecret.arn, zethRpcUrlSecret.arn, boundlessRpcUrlSecret.arn, orderStreamUrlSecret.arn],
          },
        ],
      },
    });

    const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, { name: serviceName });

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

    const deadLetterQueue = new aws.sqs.Queue(`${serviceName}-dlq`, {
      messageRetentionSeconds: 1209600, // 14 days
    });

    new aws.sqs.QueuePolicy(`${serviceName}-dlq-policy`, {
      queueUrl: deadLetterQueue.url,
      policy: pulumi.all([deadLetterQueue.arn]).apply(([queueArn]) =>
        JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Principal: {
                Service: 'events.amazonaws.com',
              },
              Action: 'sqs:SendMessage',
              Resource: queueArn,
            },
          ],
        })
      ),
    });

    new aws.iam.RolePolicy(`${serviceName}-event-bridge-dlq-policy`, {
      role: eventBridgeRole.id,
      policy: pulumi.all([deadLetterQueue.arn]).apply(([queueArn]) =>
        JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Action: 'sqs:SendMessage',
              Resource: queueArn,
            },
          ],
        })
      ),
    });

    const rule = new aws.cloudwatch.EventRule(`${serviceName}-schedule-rule`, {
      scheduleExpression: `rate(${scheduleMinutes} minutes)`,
    });

    // Create an ECS Task Definition for Fargate
    const fargateTask = new awsx.ecs.FargateTaskDefinition(
      `${serviceName}-task`,
      {
        container: {
          name: serviceName,
          image: image.ref,
          cpu: 1024,
          memory: 4096,
          essential: true,
          entryPoint: ['/bin/sh', '-c'],
          command: [
            `/app/order-generator-zeth --one-shot --max-retries ${retries} --interval ${interval} --min ${minPricePerMCycle} --max ${maxPricePerMCycle} --stake ${lockStake} --lock-timeout ${lockTimeout} --timeout ${timeout} --ramp-up ${rampUp} --set-verifier-address ${setVerifierAddr} --boundless-market-address ${boundlessMarketAddr}`,
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
              valueFrom: boundlessRpcUrlSecret.arn,
            },
            {
              name: 'ZETH_RPC_URL',
              valueFrom: zethRpcUrlSecret.arn,
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
      deadLetterConfig: {
        arn: deadLetterQueue.arn,
      },
    });

    // Log metric filters for errors. Cloudwatch metric filters do not support having multiple
    // optional patterns to match while also excluding terms. So we need to create two filters to capture
    // both "ERROR" (normal log from the service) and "error" (logged on rpc error)
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-error-filter`, {
      name: `${serviceName}-log-err-filter`,
      logGroupName: serviceName,
      metricTransformation: {
        namespace: `Bonsai/Services/${serviceName}`,
        name: `${serviceName}-log-err`,
        value: '1',
        defaultValue: '0',
      },
      // Match logs with ERROR, but without debug_storageRangeAt in the log.
      // Hitting errors on debug_storageRangeAt is a known issue with Zeth that means
      // that some blocks can't be proved.
      pattern: 'ERROR -debug_storageRangeAt',
    }, { dependsOn: [fargateTask] });

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-error-filter-2`, {
      name: `${serviceName}-log-err-filter-2`,
      logGroupName: serviceName,
      metricTransformation: {
        namespace: `Bonsai/Services/${serviceName}`,
        name: `${serviceName}-log-err-2`,
        value: '1',
        defaultValue: '0',
      },
      // Match logs with error, but without debug_storageRangeAt in the log.
      // Hitting errors on debug_storageRangeAt is a known issue with Zeth that means
      // that some blocks can't be proved.
      pattern: 'error -debug_storageRangeAt',
    }, { dependsOn: [fargateTask] });

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
      evaluationPeriods: 60,
      datapointsToAlarm: 2,
      treatMissingData: 'notBreaching',
      alarmDescription: 'Zeth order generator log ERROR level',
      actionsEnabled: true,
      alarmActions,
    });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-error-alarm-2`, {
      name: `${serviceName}-log-err-2`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: `Boundless/Services/${serviceName}`,
            metricName: `${serviceName}-log-err-2`,
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
      alarmDescription: 'Zeth order generator log ERROR level',
      actionsEnabled: true,
      alarmActions,
    });
  }
}
