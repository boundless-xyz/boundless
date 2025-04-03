import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { getEnvVar } from "./util/env";

export = () => {
  // Read config
  const config = new pulumi.Config();

  const isDev = pulumi.getStack() === "dev";
  const prefix = isDev ? `${getEnvVar("DEV_NAME")}-` : "";
  const serviceName = `${prefix}bonsai-prover`;

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
          Resource: [privateKeySecret.arn, bonsaiSecret.arn],
        },
      ],
    },
  });

  const execRole = new aws.iam.Role(`exec-${serviceName}`, {
    assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
      Service: 'ecs-tasks.amazonaws.com',
    }),
  });

  const _execRolePolicy = new aws.iam.RolePolicy(`exec-${serviceName}`, {
    role: execRole.id,
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
            'logs:CreateLogStream',
            'logs:PutLogEvents',
          ],
          Resource: '*',
        },
        {
          Effect: 'Allow',
          Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
          Resource: [privateKeySecret.arn, bonsaiSecret.arn],
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

  const authToken = aws.ecr.getAuthorizationTokenOutput({
    registryId: brokerEcr.repository.registryId,
  });

  // Optionally add in the sccache s3 creds to the build ctx
  let buildSecrets = {};
  if (ciCacheSecret !== undefined) {
    const cacheFileData = ciCacheSecret.apply((filePath: any) => fs.readFileSync(filePath, 'utf8'));
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

  // TODO use this to build? Can this be used in ec2?
  // const image = new docker_build.Image(serviceName, {
  //   tags: [dockerTagPath],
  //   context: {
  //     location: dockerDir,
  //   },
  //   platforms: ['linux/amd64'],
  //   push: true,
  //   dockerfile: {
  //     location: `${dockerDir}/dockerfiles/broker.dockerfile`,
  //   },
  //   buildArgs: {
  //     S3_CACHE_PREFIX: 'private/boundless/rust-cache-docker-Linux-X64/sccache',
  //   },
  //   secrets: buildSecrets,
  //   cacheFrom: [
  //     {
  //       registry: {
  //         ref: pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:cache`,
  //       },
  //     },
  //   ],
  //   cacheTo: [
  //     {
  //       registry: {
  //         imageManifest: true,
  //         ociMediaTypes: true,
  //         ref: pulumi.interpolate`${brokerEcr.repository.repositoryUrl}:cache`,
  //       },
  //     },
  //   ],
  //   registries: [
  //     {
  //       address: brokerEcr.repository.repositoryUrl,
  //       password: authToken.password,
  //       username: authToken.userName,
  //     },
  //   ],
  // });

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

  // Set up CloudWatch logging for the EC2 instance
  const brokerLogGroup = new aws.cloudwatch.LogGroup(serviceName, {
    name: serviceName,
    retentionInDays: 0,
    skipDestroy: true,
  });


  // Create an EC2 instance
  const ec2Instance = new aws.ec2.Instance(serviceName, {
    ami: "ami-087f352c165340ea1", // Free tier x86_64
    instanceType: "t2.micro", // Free tier instance type
    subnetId: privSubNetIds[0], // Select an appropriate subnet
    vpcSecurityGroupIds: [brokerSecGroup.id],
    // iamInstanceProfile: ecsExecPolicyArn,
    rootBlockDevice: {
        volumeSize: 50, // Size in GiB
        volumeType: 'gp3',
    },
    tags: {
        Name: serviceName,
    },
  });

  // Create an EBS volume
  const ebsVolume = new aws.ebs.Volume(serviceName, {
    availabilityZone: ec2Instance.availabilityZone,
    size: 100, // Size in GiB
    type: 'gp3',
    encrypted: true,
    tags: {
        Name: serviceName,
    },
  });

  // Attach the EBS volume to the EC2 instance
  const volumeAttachment = new aws.ec2.VolumeAttachment(serviceName, {
    instanceId: ec2Instance.id,
    volumeId: ebsVolume.id,
    // TODO likely incorrect
    deviceName: '/dev/sdh',
  });

  // User data script to configure the instance on launch
  // TODO completely untested and possibly unsafe. Also doesn't seem to use auth token for ECR?
  const userData = `#!/bin/bash
  # Install Docker
  yum update -y
  yum install -y docker
  service docker start
  usermod -a -G docker ec2-user

  # Retrieve secrets and set up environment variables
  aws secretsmanager get-secret-value --secret-id ${privateKeySecret.id} --query SecretString --output text > /home/ec2-user/private_key
  aws secretsmanager get-secret-value --secret-id ${bonsaiSecret.id} --query SecretString --output text > /home/ec2-user/bonsai_api_key

  # Run the Docker container
  docker run -d --name ${serviceName} \\
  -e PRIVATE_KEY=$(cat /home/ec2-user/private_key) \\
  -e BONSAI_API_KEY=$(cat /home/ec2-user/bonsai_api_key) \\
  -e BONSAI_API_URL=${bonsaiApiUrl} \\
  -e RPC_URL=${ethRpcUrl} \\
  -e NO_COLOR=1 \\
  -e RUST_LOG=broker=debug,boundless_market=debug \\
  -e RUST_BACKTRACE=1 \\
  -v /mnt/data:/app/data \\
  ${dockerTagPath} \\
  /bin/sh -c "/usr/bin/aws s3 cp s3://boundless-${serviceName}/broker.toml /app/broker.toml && /app/broker --set-verifier-addr ${setVerifierAddr} --boundless-market-addr ${proofMarketAddr} --order-stream-url ${orderStreamUrl} --config-file /app/broker.toml --db-url sqlite:///app/data/broker.db"
  `;

  // Update the EC2 instance with the user data script
  const _ec2InstanceWithUserData = new aws.ec2.Instance(serviceName, {
    ...ec2Instance,
    userData: userData,
  }, { dependsOn: [volumeAttachment] });

  // CloudWatch agent configuration
  const _cloudwatchAgentConfig = new aws.ssm.Parameter(`${serviceName}-cwagent-config`, {
    name: `/cloudwatch-agent/${serviceName}/config`,
    type: "String",
    value: JSON.stringify({
      logs: {
        logs_collected: {
          files: {
            collect_list: [
              {
                file_path: "/var/log/syslog",
                log_group_name: brokerLogGroup.name,
                log_stream_name: "{instance_id}/syslog",
              },
              {
                file_path: "/var/log/docker/broker.log",
                log_group_name: brokerLogGroup.name,
                log_stream_name: "{instance_id}/broker",
              },
            ],
          },
        },
      },
    }),
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
  }, { dependsOn: [ec2Instance] });

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
  }), { dependsOn: [ec2Instance] };
};