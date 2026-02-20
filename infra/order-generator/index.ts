import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import { getEnvVar, getServiceNameV1 } from '../util';
import { OrderGenerator } from './components/order-generator';
import { LoadTestTrigger } from './components/load-test';

require('dotenv').config();

export = () => {
  const stackName = pulumi.getStack();
  const isDev = stackName.includes("dev");

  const baseConfig = new pulumi.Config("order-generator-base");
  const chainId = isDev ? getEnvVar("CHAIN_ID") : baseConfig.require('CHAIN_ID');
  const pinataJWT = isDev ? pulumi.output(getEnvVar("PINATA_JWT")) : baseConfig.requireSecret('PINATA_JWT');
  const ethRpcUrl = isDev ? pulumi.output(getEnvVar("ETH_RPC_URL")) : baseConfig.requireSecret('ETH_RPC_URL');
  const indexerUrl = isDev ? pulumi.output(getEnvVar("INDEXER_URL")) : baseConfig.requireSecret('INDEXER_URL');
  const orderStreamUrl = isDev
    ? pulumi.output(getEnvVar("ORDER_STREAM_URL"))
    : (baseConfig.getSecret('ORDER_STREAM_URL') || pulumi.output(""));
  const githubTokenSecret = baseConfig.getSecret('GH_TOKEN_SECRET');
  const logLevel = baseConfig.get('LOG_LEVEL') || 'info';
  const dockerDir = baseConfig.get('DOCKER_DIR') || '../../';
  const dockerTag = baseConfig.get('DOCKER_TAG') || 'latest';
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;
  const setVerifierAddr = baseConfig.require('SET_VERIFIER_ADDR');
  const boundlessMarketAddr = baseConfig.require('BOUNDLESS_MARKET_ADDR');
  const ipfsGateway = baseConfig.get('IPFS_GATEWAY_URL') || 'https://dweb.link';
  const baseStackName = baseConfig.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
  const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
  const boundlessAlertsTopicArn = baseConfig.get('SLACK_ALERTS_TOPIC_ARN');
  const boundlessPagerdutyTopicArn = baseConfig.get('PAGERDUTY_ALERTS_TOPIC_ARN');
  const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];
  const interval = baseConfig.get('INTERVAL') || '60';
  const txTimeout = baseConfig.get('TX_TIMEOUT') || '180';

  const imageName = getServiceNameV1(stackName, `order-generator`);
  const repo = new awsx.ecr.Repository(`${imageName}-ecr-repo`, {
    name: `${imageName}-ecr-repo`,
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
      githubTokenSecret
    }
  }

  const dockerTagPath = pulumi.interpolate`${repo.repository.repositoryUrl}:${dockerTag}`;

  const image = new docker_build.Image(`${imageName}-image`, {
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
      location: `${dockerDir}/dockerfiles/order_generator.dockerfile`,
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

  const offchainConfig = new pulumi.Config("order-generator-offchain");
  const offchainAutoDeposit = offchainConfig.get('AUTO_DEPOSIT');
  const offchainWarnBalanceBelow = offchainConfig.get('WARN_BALANCE_BELOW');
  const offchainErrorBalanceBelow = offchainConfig.get('ERROR_BALANCE_BELOW');
  let offchainPrivateKey: pulumi.Output<string> | undefined;
  if (isDev && process.env.OFFCHAIN_PRIVATE_KEY) {
    offchainPrivateKey = pulumi.output(process.env.OFFCHAIN_PRIVATE_KEY);
  } else {
    offchainPrivateKey = offchainConfig.requireSecret('PRIVATE_KEY');
  }
  const offchainInputMaxMCycles = offchainConfig.get('INPUT_MAX_MCYCLES') ?? "1000";
  const offchainInterval = offchainConfig.get('INTERVAL');
  const offchainMaxPriceCap = offchainConfig.get('MAX_PRICE_CAP');

  if (offchainPrivateKey) {
    new OrderGenerator('offchain', {
      chainId,
      stackName,
      privateKey: offchainPrivateKey,
      pinataJWT,
      ethRpcUrl,
      autoDeposit: offchainAutoDeposit,
      warnBalanceBelow: offchainWarnBalanceBelow,
      errorBalanceBelow: offchainErrorBalanceBelow,
      offchainConfig: {
        orderStreamUrl,
      },
      image,
      logLevel,
      setVerifierAddr,
      boundlessMarketAddr,
      ipfsGateway,
      interval: offchainInterval ?? interval,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      inputMaxMCycles: offchainInputMaxMCycles,
      indexerUrl,
      useZeth: false,
      maxPriceCap: offchainMaxPriceCap,
    });
  }

  const onchainConfig = new pulumi.Config("order-generator-onchain");
  const onchainAutoDeposit = onchainConfig.get('AUTO_DEPOSIT');
  const onchainWarnBalanceBelow = onchainConfig.get('WARN_BALANCE_BELOW');
  const onchainErrorBalanceBelow = onchainConfig.get('ERROR_BALANCE_BELOW');
  let onchainPrivateKey: pulumi.Output<string> | undefined;
  if (isDev && process.env.ONCHAIN_PRIVATE_KEY) {
    onchainPrivateKey = pulumi.output(process.env.ONCHAIN_PRIVATE_KEY);
  } else {
    onchainPrivateKey = onchainConfig.requireSecret('PRIVATE_KEY');
  }
  const onchainInputMaxMCycles = onchainConfig.get('INPUT_MAX_MCYCLES');
  const onchainInterval = onchainConfig.get('INTERVAL');
  const onchainMaxPriceCap = onchainConfig.get('MAX_PRICE_CAP');

  if (onchainPrivateKey) {
    new OrderGenerator('onchain', {
      chainId,
      stackName,
      autoDeposit: onchainAutoDeposit,
      warnBalanceBelow: onchainWarnBalanceBelow,
      errorBalanceBelow: onchainErrorBalanceBelow,
      privateKey: onchainPrivateKey,
      pinataJWT,
      ethRpcUrl,
      image,
      logLevel,
      setVerifierAddr,
      boundlessMarketAddr,
      ipfsGateway,
      interval: onchainInterval ?? interval,
      inputMaxMCycles: onchainInputMaxMCycles,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      indexerUrl,
      useZeth: false,
      maxPriceCap: onchainMaxPriceCap,
    });
  }

  const randomRequestorConfig = new pulumi.Config("order-generator-random-requestor");
  let randomRequestorPrivateKey: pulumi.Output<string> | undefined;
  let randomRequestorMnemonic: pulumi.Output<string> | undefined;
  let randomRequestorDeriveRotationKeys: number | undefined;
  if (isDev && process.env.RANDOM_REQUESTOR_PRIVATE_KEY) {
    randomRequestorPrivateKey = pulumi.output(process.env.RANDOM_REQUESTOR_PRIVATE_KEY);
  } else if (isDev && process.env.RANDOM_REQUESTOR_MNEMONIC && process.env.RANDOM_REQUESTOR_DERIVE_ROTATION_KEYS) {
    randomRequestorMnemonic = pulumi.output(process.env.RANDOM_REQUESTOR_MNEMONIC);
    const parsed = parseInt(process.env.RANDOM_REQUESTOR_DERIVE_ROTATION_KEYS, 10);
    randomRequestorDeriveRotationKeys = Number.isInteger(parsed) && parsed >= 2 ? parsed : undefined;
  } else {
    randomRequestorPrivateKey = randomRequestorConfig.getSecret('PRIVATE_KEY');
    randomRequestorMnemonic = randomRequestorConfig.getSecret('MNEMONIC');
    const n = randomRequestorConfig.get('DERIVE_ROTATION_KEYS');
    const parsed = n ? parseInt(n, 10) : NaN;
    randomRequestorDeriveRotationKeys = Number.isInteger(parsed) && parsed >= 2 ? parsed : undefined;
  }

  const randomRequestorUsesRotation =
    !!randomRequestorMnemonic && !!randomRequestorDeriveRotationKeys && randomRequestorDeriveRotationKeys >= 2;
  const randomRequestorDeploy = randomRequestorPrivateKey || randomRequestorUsesRotation;

  if (randomRequestorDeploy) {
    const randomRequestorInterval = randomRequestorConfig.get('INTERVAL') ?? "180";
    const randomRequestorCount = randomRequestorConfig.get('COUNT') ?? "1";
    const randomRequestorScheduleExpression = randomRequestorConfig.get('SCHEDULE_EXPRESSION') ?? "rate(24 hours)";
    const randomRequestorAutoDeposit = randomRequestorConfig.get('AUTO_DEPOSIT');
    const randomRequestorWarnBalanceBelow = randomRequestorConfig.get('WARN_BALANCE_BELOW');
    const randomRequestorErrorBalanceBelow = randomRequestorConfig.get('ERROR_BALANCE_BELOW');
    const randomRequestorInputMaxMCycles = randomRequestorConfig.get('INPUT_MAX_MCYCLES');
    const randomRequestorMaxPriceCap = randomRequestorConfig.get('MAX_PRICE_CAP');

    new OrderGenerator('random-requestor', {
      chainId,
      stackName,
      ...(randomRequestorUsesRotation
        ? { rotationConfig: { mnemonic: randomRequestorMnemonic!, deriveRotationKeys: randomRequestorDeriveRotationKeys! } }
        : { privateKey: randomRequestorPrivateKey! }),
      pinataJWT,
      ethRpcUrl,
      autoDeposit: randomRequestorAutoDeposit,
      warnBalanceBelow: randomRequestorWarnBalanceBelow,
      errorBalanceBelow: randomRequestorErrorBalanceBelow,
      image,
      logLevel,
      setVerifierAddr,
      boundlessMarketAddr,
      ipfsGateway,
      interval: randomRequestorInterval,
      inputMaxMCycles: randomRequestorInputMaxMCycles,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      oneShotConfig: {
        count: randomRequestorCount,
        scheduleExpression: randomRequestorScheduleExpression,
      },
      indexerUrl,
      useZeth: false,
      maxPriceCap: randomRequestorMaxPriceCap,
    });
  }

  const evmRequestorConfig = new pulumi.Config("order-generator-evm-requestor");
  const evmRequestorAutoDeposit = evmRequestorConfig.get('AUTO_DEPOSIT');
  const evmRequestorWarnBalanceBelow = evmRequestorConfig.get('WARN_BALANCE_BELOW');
  const evmRequestorErrorBalanceBelow = evmRequestorConfig.get('ERROR_BALANCE_BELOW');
  let evmRequestorPrivateKey: pulumi.Output<string> | undefined;
  let evmRequestorMnemonic: pulumi.Output<string> | undefined;
  let evmRequestorDeriveRotationKeys: number | undefined;
  if (isDev && process.env.EVM_REQUESTOR_PRIVATE_KEY) {
    evmRequestorPrivateKey = pulumi.output(process.env.EVM_REQUESTOR_PRIVATE_KEY);
  } else if (isDev && process.env.EVM_REQUESTOR_MNEMONIC && process.env.EVM_REQUESTOR_DERIVE_ROTATION_KEYS) {
    evmRequestorMnemonic = pulumi.output(process.env.EVM_REQUESTOR_MNEMONIC);
    const parsed = parseInt(process.env.EVM_REQUESTOR_DERIVE_ROTATION_KEYS, 10);
    evmRequestorDeriveRotationKeys = Number.isInteger(parsed) && parsed >= 2 ? parsed : undefined;
  } else {
    evmRequestorPrivateKey = evmRequestorConfig.getSecret('PRIVATE_KEY');
    evmRequestorMnemonic = evmRequestorConfig.getSecret('MNEMONIC');
    const n = evmRequestorConfig.get('DERIVE_ROTATION_KEYS');
    const parsed = n ? parseInt(n, 10) : NaN;
    evmRequestorDeriveRotationKeys = Number.isInteger(parsed) && parsed >= 2 ? parsed : undefined;
  }
  const evmRequestorUsesRotation =
    !!evmRequestorMnemonic && !!evmRequestorDeriveRotationKeys && evmRequestorDeriveRotationKeys >= 2;
  const evmRequestorDeploy = evmRequestorPrivateKey || evmRequestorUsesRotation;
  const evmRequestorInterval = evmRequestorConfig.get('INTERVAL');
  const evmRequestorInputMaxMCycles = evmRequestorConfig.get('INPUT_MAX_MCYCLES');
  const evmRequestorMaxPriceCap = evmRequestorConfig.get('MAX_PRICE_CAP');

  const evmS3Bucket = evmRequestorConfig.get('S3_BUCKET');
  const evmS3Region = evmRequestorConfig.get('AWS_REGION');
  const evmS3AccessKeyId = evmRequestorConfig.getSecret('S3_ACCESS_KEY_ID');
  const evmS3SecretAccessKey = evmRequestorConfig.getSecret('S3_SECRET_ACCESS_KEY');
  const evmS3Config = evmS3Bucket && evmS3Region && evmS3AccessKeyId && evmS3SecretAccessKey
    ? { bucket: evmS3Bucket, region: evmS3Region, accessKeyId: evmS3AccessKeyId, secretAccessKey: evmS3SecretAccessKey }
    : undefined;

  if (evmRequestorDeploy) {
    new OrderGenerator('evm-requestor', {
      chainId,
      stackName,
      autoDeposit: evmRequestorAutoDeposit,
      warnBalanceBelow: evmRequestorWarnBalanceBelow,
      errorBalanceBelow: evmRequestorErrorBalanceBelow,
      ...(evmRequestorUsesRotation
        ? { rotationConfig: { mnemonic: evmRequestorMnemonic!, deriveRotationKeys: evmRequestorDeriveRotationKeys! } }
        : { privateKey: evmRequestorPrivateKey! }),
      ...(evmS3Config ? { s3Config: evmS3Config } : { pinataJWT }),
      ethRpcUrl,
      image,
      logLevel,
      setVerifierAddr,
      boundlessMarketAddr,
      ipfsGateway,
      interval: evmRequestorInterval ?? interval,
      inputMaxMCycles: evmRequestorInputMaxMCycles,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      indexerUrl,
      useZeth: true,
      maxPriceCap: evmRequestorMaxPriceCap,
    });
  }

  const loadTestConfig = new pulumi.Config("order-generator-load-test");
  let loadTestPrivateKey: pulumi.Output<string> | undefined;
  if (isDev && process.env.LOAD_TEST_PRIVATE_KEY) {
    loadTestPrivateKey = pulumi.output(process.env.LOAD_TEST_PRIVATE_KEY);
  } else {
    loadTestPrivateKey = loadTestConfig.getSecret('PRIVATE_KEY');
  }

  if (loadTestPrivateKey) {
    new LoadTestTrigger({
      chainId,
      stackName,
      privateKey: loadTestPrivateKey,
      pinataJWT,
      ethRpcUrl,
      indexerUrl,
      orderStreamUrl,
      image,
      logLevel,
      setVerifierAddr: setVerifierAddr,
      boundlessMarketAddr: boundlessMarketAddr,
      ipfsGateway,
      lockCollateralRaw,
      vpcId,
      privateSubnetIds,
    });
  }
};
