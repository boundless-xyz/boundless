import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import { config } from 'process';
import { getEnvVar, ChainId, getServiceNameV1, Severity } from "../../util";
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as fs from 'fs';
import { createProverAlarms } from './brokerAlarms';
import * as crypto from 'crypto';

export class BonsaiECSBroker extends pulumi.ComponentResource {
  constructor(name: string, args: {
    chainId: string;
    privateKey: string | pulumi.Output<string>;
    ethRpcUrl: string | pulumi.Output<string>;
    orderStreamUrl: string | pulumi.Output<string>;
    baseStackName: string;
    vpcId: pulumi.Output<any>;
    privSubNetIds: pulumi.Output<any>;
    dockerDir: string;
    dockerTag: string;
    setVerifierAddr: string;
    boundlessMarketAddr: string;
    bonsaiApiUrl: string;
    bonsaiApiKey: string | pulumi.Output<string> | undefined;
    ciCacheSecret?: pulumi.Output<string>;
    githubTokenSecret?: pulumi.Output<string>;
    brokerTomlPath: string;
    boundlessAlertsTopicArns?: string[];
    dockerRemoteBuilder?: string;
  }, opts?: pulumi.ComponentResourceOptions) {
    super(`${name}-${args.chainId}`, name, opts);

    const isDev = pulumi.getStack() === "dev";

    const {
      ethRpcUrl,
      privateKey,
      bonsaiApiUrl,
      dockerRemoteBuilder,
      bonsaiApiKey,
      orderStreamUrl,
      brokerTomlPath,
      boundlessMarketAddr: proofMarketAddr,
      setVerifierAddr,
      vpcId,
      privSubNetIds,
      dockerDir,
      dockerTag,
      ciCacheSecret,
      githubTokenSecret,
      boundlessAlertsTopicArns
    } = args;
    const serviceName = name;

    const ethRpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerEthRpc`);
    const _ethRpcUrlSecretSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-brokerEthRpc`, {
      secretId: ethRpcUrlSecret.id,
      secretString: ethRpcUrl,
    });

    const privateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-brokerPrivateKey`);
    const _privateKeySecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-privateKeyValue`, {
      secretId: privateKeySecret.id,
      secretString: privateKey,
    });

    const bonsaiSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerBonsaiKey`);
    const _bonsaiSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-bonsaiKeyValue`, {
      secretId: bonsaiSecret.id,
      secretString: bonsaiApiKey,
    });

    const orderStreamUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-brokerOrderStreamUrl`);
    const _orderStreamUrlSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-brokerOrderStreamUrl`, {
      secretId: orderStreamUrlSecret.id,
      secretString: orderStreamUrl,
    });

    // Hash the secret strings. 
    // This is used to determine if the secrets have changed and trigger a redeployment of the ECS task.
    // Necessary because the secrets are passed as secret manager arns to the ECS task, and the arn doesnt change,
    // so Pulumi is unable to tell if the value stored within secret manager has changed at deployment time.
    const secretHash = pulumi
      .all([ethRpcUrl, privateKey, orderStreamUrl, bonsaiApiKey])
      .apply(([_ethRpcUrl, _privateKey, _orderStreamUrl, _bonsaiApiKey]) => {
        const hash = crypto.createHash("sha1");
        hash.update(_ethRpcUrl);
        hash.update(_privateKey);
        hash.update(_orderStreamUrl);
        hash.update(_bonsaiApiKey ?? '');
        return hash.digest("hex");
      });

    const brokerS3Bucket = new aws.s3.Bucket(serviceName, {
      bucketPrefix: serviceName.substring(0, 35), // Restrict to max length of the bucket prefix
      tags: {
        Name: serviceName,
      },
    });

    const fileToUpload = new pulumi.asset.FileAsset(brokerTomlPath);

    const bucketObject = new aws.s3.BucketObject(serviceName, {
      bucket: brokerS3Bucket.id,
      key: 'broker.toml',
      source: fileToUpload,
    });

    // EFS
    const fileSystem = new aws.efs.FileSystem(`${serviceName}-efs-rev5`, {
      encrypted: true,
      tags: {
        Name: serviceName,
      },
    });

    const mountTargets = privSubNetIds.apply((subnets) =>
      subnets.map((subnetId: string, index: number) => {
        return new aws.efs.MountTarget(`${serviceName}-mount-${index}`, {
          fileSystemId: fileSystem.id,
          subnetId: subnetId,
          securityGroups: [brokerSecGroup.id],
        }, { dependsOn: [fileSystem] });
      })
    );

    const taskRole = new aws.iam.Role(`${serviceName}-task-role`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
        Service: 'ecs-tasks.amazonaws.com',
      }),
    });

    const _rolePolicy = new aws.iam.RolePolicy(`${serviceName}-role-policy`, {
      role: taskRole.id,
      policy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['s3:GetObject', 's3:ListObject', 's3:HeadObject'],
            Resource: [bucketObject.arn],
          },
          {
            Effect: 'Allow',
            Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
            Resource: [privateKeySecret.arn, bonsaiSecret.arn, ethRpcUrlSecret.arn, orderStreamUrlSecret.arn],
          },
          // Needed for ECS Exec
          {
            Effect: 'Allow',
            Action: [
              "ssmmessages:CreateControlChannel",
              "ssmmessages:CreateDataChannel",
              "ssmmessages:OpenControlChannel",
              "ssmmessages:OpenDataChannel"
            ],
            Resource: "*"
          }
        ],
      },
    }, { dependsOn: [taskRole] });

    const brokerEcr = new awsx.ecr.Repository(`${serviceName}-ecr`, {
      lifecyclePolicy: {
        rules: [
          {
            description: 'Delete untagged images after N days',
            tagStatus: 'untagged',
            maximumAgeLimit: 7,
          },
        ],
      },
      forceDelete: true,
    });

    const executionRole = new aws.iam.Role(`${serviceName}-ecsrole`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
        Service: 'ecs-tasks.amazonaws.com',
      }),
    });

    pulumi.all([brokerEcr.repository.arn, executionRole.name]).apply(([ecrRepoArn, executionRoleName]) => {
      new aws.iam.RolePolicy(`${serviceName}-ecs-execution-pol`, {
        role: executionRoleName,
        policy: {
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Action: [
                'ecr:GetAuthorizationToken',
                'ecr:BatchCheckLayerAvailability',
                'ecr:GetDownloadUrlForLayer',
                'ecr:BatchGetImage',
              ],
              Resource: '*',
            },
            {
              Effect: 'Allow',
              Action: [
                'logs:CreateLogStream',
                'logs:PutLogEvents',
              ],
              Resource: '*',
            },
            {
              Effect: 'Allow',
              Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
              Resource: [privateKeySecret.arn, bonsaiSecret.arn, ethRpcUrlSecret.arn, orderStreamUrlSecret.arn],
            },
          ],
        },
      }, { dependsOn: [executionRole] });
    })

    const authToken = aws.ecr.getAuthorizationTokenOutput({
      registryId: brokerEcr.repository.registryId,
    });

    // Optionally add in the gh token secret and sccache s3 creds to the build ctx
    let buildSecrets = {};
    if (ciCacheSecret !== undefined) {
      const cacheFileData = ciCacheSecret.apply((filePath) => fs.readFileSync(filePath, 'utf8'));
      buildSecrets = {
        ci_cache_creds: cacheFileData,
      };
    }
    if (githubTokenSecret !== undefined) {
      buildSecrets = {
        ...buildSecrets,
        githubTokenSecret
      }
    }

    const dockerTagPath = pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:${dockerTag}`;

    const image = new docker_build.Image(serviceName, {
      tags: [dockerTagPath],
      context: {
        location: dockerDir,
      },
      platforms: ['linux/amd64'],
      push: true,
      builder: dockerRemoteBuilder ? {
        name: dockerRemoteBuilder,
      } : undefined,
      dockerfile: {
        location: `${dockerDir}/dockerfiles/${isDev ? 'dev/' : ''}broker.dockerfile`,
      },
      buildArgs: {
        S3_CACHE_PREFIX: 'private/boundless/rust-cache-docker-Linux-X64/sccache',
      },
      secrets: buildSecrets,
      cacheFrom: [
        {
          registry: {
            ref: pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:cache`,
          },
        },
      ],
      cacheTo: [
        {
          registry: {
            mode: docker_build.CacheMode.Max,
            imageManifest: true,
            ociMediaTypes: true,
            ref: pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:cache`,
          },
        },
      ],
      registries: [
        {
          address: brokerEcr.repository.repositoryUrl,
          password: authToken.password,
          username: authToken.userName,
        },
      ],
    });

    const cluster = new aws.ecs.Cluster(serviceName, {
      name: serviceName,
    });

    const brokerSecGroup = new aws.ec2.SecurityGroup(serviceName, {
      name: serviceName,
      vpcId: vpcId,
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

    const _efsInbound = new aws.ec2.SecurityGroupRule(`${serviceName}-efs-inbound`, {
      type: 'ingress',
      fromPort: 2049,
      toPort: 2049,
      protocol: 'tcp',
      securityGroupId: brokerSecGroup.id,
      sourceSecurityGroupId: brokerSecGroup.id,
    });

    const brokerS3BucketName = brokerS3Bucket.bucket.apply(n => n);

    // Try to get existing log group
    const existingLogGroup = pulumi.output(aws.cloudwatch.getLogGroup({
      name: serviceName,
    }).catch(() => undefined));

    const logGroup = existingLogGroup.apply(existing => {
      if (existing) {
        // Convert the existing log group to a LogGroup resource
        return new aws.cloudwatch.LogGroup(`${serviceName}-log-group`, {
          name: existing.name,
          retentionInDays: existing.retentionInDays,
        }, { parent: this, import: existing.id });
      }
      return new aws.cloudwatch.LogGroup(`${serviceName}-log-group`, {
        name: serviceName,
        retentionInDays: 0,
      }, { parent: this });
    });


    const service = new awsx.ecs.FargateService(serviceName, {
      name: serviceName,
      cluster: cluster.arn,
      networkConfiguration: {
        securityGroups: [brokerSecGroup.id],
        assignPublicIp: false,
        subnets: privSubNetIds,
      },
      deploymentCircuitBreaker: {
        enable: false,
        rollback: true,
      },
      desiredCount: 1,
      // These min/maxs prevent 2 tasks from being run in parallel
      // because broker is a singleton but FARGATE does not support DAEMON strategy
      deploymentMinimumHealthyPercent: 0,
      deploymentMaximumPercent: 100,
      enableExecuteCommand: true,
      taskDefinitionArgs: {
        family: 'broker',
        executionRole: {
          roleArn: executionRole.arn,
        },
        taskRole: {
          roleArn: taskRole.arn,
        },
        logGroup: {
          existing: {
            name: logGroup.name,
          },
        },
        volumes: [
          {
            name: 'broker-storage',
            efsVolumeConfiguration: {
              fileSystemId: fileSystem.id,
              rootDirectory: '/',
            },
          },
        ],
        container: {
          name: serviceName,
          image: image.ref,
          mountPoints: [
            {
              sourceVolume: 'broker-storage',
              containerPath: '/app/data',
              readOnly: false,
            },
          ],
          entryPoint: ['/bin/sh', '-c'],
          cpu: 4096,
          memory: 8192,
          essential: true,
          linuxParameters: {
            initProcessEnabled: true,
          },
          command: [
            `/usr/bin/aws s3 cp s3://$BUCKET/broker.toml /app/broker.toml && /app/broker --set-verifier-address ${setVerifierAddr} --boundless-market-address ${proofMarketAddr} --config-file /app/broker.toml --db-url sqlite:///app/data/broker.db`,
          ],
          secrets: [
            {
              name: 'PRIVATE_KEY',
              valueFrom: privateKeySecret.arn,
            },
            {
              name: 'BONSAI_API_KEY',
              valueFrom: bonsaiSecret.arn,
            },
            {
              name: 'RPC_URL',
              valueFrom: ethRpcUrlSecret.arn,
            },
            {
              name: 'ORDER_STREAM_URL',
              valueFrom: orderStreamUrlSecret.arn,
            }
          ],
          environment: [
            { name: 'NO_COLOR', value: '1' },
            { name: 'RUST_LOG', value: 'broker=debug,boundless_market=debug' },
            { name: 'RUST_BACKTRACE', value: '1' },
            { name: 'BONSAI_API_URL', value: bonsaiApiUrl },
            { name: 'BUCKET', value: brokerS3BucketName },
            { name: 'SECRET_HASH', value: secretHash },
          ],
        },
      },
    }, { dependsOn: [fileSystem, mountTargets] });

    const alarmActions = boundlessAlertsTopicArns ?? [];

    createProverAlarms(serviceName, logGroup, [service, logGroup], alarmActions);

    // Alarms for CPUUtilization and MemoryUtilization, alarm if over 80% for 5 consecutive minutes.
    new aws.cloudwatch.MetricAlarm(`${serviceName}-cpu-utilization-alarm`, {
      name: `${serviceName}-cpu-utilization-alarm`,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 5,
      datapointsToAlarm: 5,
      metricName: 'CPUUtilization',
      namespace: 'AWS/ECS',
      period: 60,
      statistic: 'Average',
      threshold: 80,
      alarmDescription: 'This metric monitors the CPU utilization of the broker task.',
      alarmActions: alarmActions,
      dimensions: {
        ServiceName: serviceName,
        ClusterName: cluster.name,
      },
    });

    new aws.cloudwatch.MetricAlarm(`${serviceName}-memory-utilization-alarm`, {
      name: `${serviceName}-memory-utilization-alarm`,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      metricName: 'MemoryUtilization',
      namespace: 'AWS/ECS',
      period: 60,
      evaluationPeriods: 5,
      datapointsToAlarm: 5,
      statistic: 'Average',
      threshold: 80,
      alarmDescription: 'This metric monitors the memory utilization of the broker task.',
      alarmActions: alarmActions,
      dimensions: {
        ServiceName: serviceName,
        ClusterName: cluster.name,
      },
    });

  }
}
