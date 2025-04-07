import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { getEnvVar, ChainId, getServiceName } from "../util";

export = () => {
  // Read config
  const config = new pulumi.Config();

  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const serviceName = getServiceName(stackName, "bonsai-prover", ChainId.SEPOLIA);
  
  const baseStackName = config.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');

  const setVerifierAddr = config.require('SET_VERIFIER_ADDR');
  const proofMarketAddr = config.require('PROOF_MARKET_ADDR');
  const privateKey = isDev ? getEnvVar("PRIVATE_KEY") : config.requireSecret('PRIVATE_KEY');
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');
  const bonsaiApiUrl = config.require('BONSAI_API_URL');
  const bonsaiApiKey = isDev ? getEnvVar("BONSAI_API_KEY") : config.getSecret('BONSAI_API_KEY');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  const orderStreamUrl = config.require('ORDER_STREAM_URL');
  const brokerTomlPath = config.require('BROKER_TOML_PATH')
  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');

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

  const brokerS3Bucket = new aws.s3.Bucket(serviceName, {
    bucketPrefix: `boundless-${serviceName}`,
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
  const fileSystem = new aws.efs.FileSystem(serviceName, {
    encrypted: true,
    tags: {
      Name: serviceName,
    },
  });

  privSubNetIds.apply((subnets) =>
    subnets.map((subnetId: string, index: number) => {
      return new aws.efs.MountTarget(`${serviceName}-mount-${index}`, {
        fileSystemId: fileSystem.id,
        subnetId: subnetId,
        securityGroups: [brokerSecGroup.id],
      });
    })
  );

  const taskRole = new aws.iam.Role(serviceName, {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
      Service: 'ecs-tasks.amazonaws.com',
    }),
  });

  const _rolePolicy = new aws.iam.RolePolicy(serviceName, {
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
          Resource: [privateKeySecret.arn, bonsaiSecret.arn, ethRpcUrlSecret.arn],
        },
      ],
    },
  });

  const brokerEcr = new awsx.ecr.Repository(serviceName, {
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
    name: serviceName,
  });

  const executionRole = new aws.iam.Role(`${serviceName}-ecs-execution-role`, {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
      Service: 'ecs-tasks.amazonaws.com',
    }),
  });

  brokerEcr.repository.arn.apply(arn => {
    new aws.iam.RolePolicy(`${serviceName}-ecs-execution-pol`, {
      role: executionRole.id,
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
            Resource: [privateKeySecret.arn, bonsaiSecret.arn, ethRpcUrlSecret.arn],
          },
        ],
      },
    });
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
    dockerfile: {
      location: `${dockerDir}/dockerfiles/broker.dockerfile`,
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
        args: {
          name: serviceName,
          retentionInDays: 0,
          skipDestroy: true,
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
          `/usr/bin/aws s3 cp s3://$BUCKET/broker.toml /app/broker.toml && /app/broker --set-verifier-address ${setVerifierAddr} --boundless-market-address ${proofMarketAddr} --order-stream-url ${orderStreamUrl} --config-file /app/broker.toml --db-url sqlite:///app/data/broker.db`,
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
          }
        ],
        environment: [
          { name: 'NO_COLOR', value: '1' },
          { name: 'RUST_LOG', value: 'broker=debug,boundless_market=debug' },
          { name: 'RUST_BACKTRACE', value: '1' },
          { name: 'BONSAI_API_URL', value: bonsaiApiUrl },
          { name: 'BUCKET', value: brokerS3BucketName }
        ],
      },
    },
  });

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

  new aws.cloudwatch.LogMetricFilter(`${serviceName}-lock-filter`, {
    name: `${serviceName}-log-lock-filter`,
    logGroupName: serviceName,
    metricTransformation: {
      namespace: `Boundless/Services/${serviceName}`,
      name: `${serviceName}-log-lock`,
      value: '1',
      defaultValue: '0',
    },
    pattern: '?"Locked order" ?"locked order" ?"Order locked"',
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
          stat: 'Maximum',
        },
        returnData: true,
      },
    ],
    threshold: 1,
    comparisonOperator: 'GreaterThanOrEqualToThreshold',
    evaluationPeriods: 1,
    datapointsToAlarm: 1,
    treatMissingData: 'notBreaching',
    alarmDescription: `ERROR log detected for ${serviceName}`,
    actionsEnabled: true,
    alarmActions,
  });
};
