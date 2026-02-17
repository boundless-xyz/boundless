import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import { getEnvVar, getServiceNameV1 } from '../util';
import { OrderGenerator } from './components/order-generator';

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
  const ciCacheSecret = baseConfig.getSecret('CI_CACHE_SECRET');
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
  const lockCollateralRaw = baseConfig.get('LOCK_COLLATERAL_RAW') || '15000000000000000000';
  const minPricePerMCycle = baseConfig.get('MIN_PRICE_PER_MCYCLE') || '0';
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
  if (ciCacheSecret !== undefined) {
    buildSecrets = {
      ci_cache_creds: ciCacheSecret,
    };
  }
  if (githubTokenSecret !== undefined) {
    buildSecrets = {
      ...buildSecrets,
      githubTokenSecret
    };
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
  const offchainRampUp = offchainConfig.get('RAMP_UP');
  const offchainLockTimeout = offchainConfig.get('LOCK_TIMEOUT');
  const offchainTimeout = offchainConfig.get('TIMEOUT');
  const offchainSecondsPerMCycle = offchainConfig.get('SECONDS_PER_MCYCLE');
  const offchainRampUpSecondsPerMCycle = offchainConfig.get('RAMP_UP_SECONDS_PER_MCYCLE');
  const offchainInterval = offchainConfig.get('INTERVAL');
  const offchainExecRateKhz = offchainConfig.get('EXEC_RATE_KHZ');
  const offchainMaxPricePerMCycle = offchainConfig.get('MAX_PRICE_PER_MCYCLE');
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
      lockCollateralRaw,
      minPricePerMCycle,
      maxPricePerMCycle: offchainMaxPricePerMCycle,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      inputMaxMCycles: offchainInputMaxMCycles,
      rampUp: offchainRampUp,
      rampUpSecondsPerMCycle: offchainRampUpSecondsPerMCycle,
      lockTimeout: offchainLockTimeout,
      timeout: offchainTimeout,
      secondsPerMCycle: offchainSecondsPerMCycle,
      execRateKhz: offchainExecRateKhz,
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
  const onchainRampUp = onchainConfig.get('RAMP_UP');
  const onchainLockTimeout = onchainConfig.get('LOCK_TIMEOUT');
  const onchainTimeout = onchainConfig.get('TIMEOUT');
  const onchainSecondsPerMCycle = onchainConfig.get('SECONDS_PER_MCYCLE');
  const onchainRampUpSecondsPerMCycle = onchainConfig.get('RAMP_UP_SECONDS_PER_MCYCLE');
  const onchainInterval = onchainConfig.get('INTERVAL');
  const onchainExecRateKhz = onchainConfig.get('EXEC_RATE_KHZ');
  const onchainMaxPricePerMCycle = onchainConfig.get('MAX_PRICE_PER_MCYCLE');
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
      lockCollateralRaw,
      rampUp: onchainRampUp,
      inputMaxMCycles: onchainInputMaxMCycles,
      minPricePerMCycle,
      maxPricePerMCycle: onchainMaxPricePerMCycle,
      secondsPerMCycle: onchainSecondsPerMCycle,
      rampUpSecondsPerMCycle: onchainRampUpSecondsPerMCycle,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      lockTimeout: onchainLockTimeout,
      timeout: onchainTimeout,
      execRateKhz: onchainExecRateKhz,
      indexerUrl,
      useZeth: false,
      maxPriceCap: onchainMaxPriceCap,
    });
  }

  const randomRequestorConfig = new pulumi.Config("order-generator-random-requestor");
  let randomRequestorPrivateKey: pulumi.Output<string> | undefined;
  if (isDev && process.env.RANDOM_REQUESTOR_PRIVATE_KEY) {
    randomRequestorPrivateKey = pulumi.output(process.env.RANDOM_REQUESTOR_PRIVATE_KEY);
  } else {
    // We only deploy the random requestor on some networks, so we don't require private key.
    randomRequestorPrivateKey = randomRequestorConfig.getSecret('PRIVATE_KEY');
  }

  if (randomRequestorPrivateKey) {
    const randomRequestorInterval = randomRequestorConfig.get('INTERVAL') ?? "180";
    const randomRequestorCount = randomRequestorConfig.get('COUNT') ?? "1";
    const randomRequestorScheduleExpression = randomRequestorConfig.get('SCHEDULE_EXPRESSION') ?? "rate(24 hours)";
    const randomRequestorAutoDeposit = randomRequestorConfig.get('AUTO_DEPOSIT');
    const randomRequestorWarnBalanceBelow = randomRequestorConfig.get('WARN_BALANCE_BELOW');
    const randomRequestorErrorBalanceBelow = randomRequestorConfig.get('ERROR_BALANCE_BELOW');
    const randomRequestorInputMaxMCycles = randomRequestorConfig.get('INPUT_MAX_MCYCLES');
    const randomRequestorRampUp = randomRequestorConfig.get('RAMP_UP');
    const randomRequestorLockTimeout = randomRequestorConfig.get('LOCK_TIMEOUT');
    const randomRequestorTimeout = randomRequestorConfig.get('TIMEOUT');
    const randomRequestorSecondsPerMCycle = randomRequestorConfig.get('SECONDS_PER_MCYCLE');
    const randomRequestorRampUpSecondsPerMCycle = randomRequestorConfig.get('RAMP_UP_SECONDS_PER_MCYCLE');
    const randomRequestorExecRateKhz = randomRequestorConfig.get('EXEC_RATE_KHZ');
    const randomRequestorMaxPricePerMCycle = randomRequestorConfig.get('MAX_PRICE_PER_MCYCLE');
    const randomRequestorMaxPriceCap = randomRequestorConfig.get('MAX_PRICE_CAP');

    new OrderGenerator('random-requestor', {
      chainId,
      stackName,
      privateKey: randomRequestorPrivateKey,
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
      lockCollateralRaw,
      rampUp: randomRequestorRampUp,
      inputMaxMCycles: randomRequestorInputMaxMCycles,
      minPricePerMCycle,
      maxPricePerMCycle: randomRequestorMaxPricePerMCycle,
      secondsPerMCycle: randomRequestorSecondsPerMCycle,
      rampUpSecondsPerMCycle: randomRequestorRampUpSecondsPerMCycle,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      lockTimeout: randomRequestorLockTimeout,
      timeout: randomRequestorTimeout,
      execRateKhz: randomRequestorExecRateKhz,
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
  if (isDev && process.env.EVM_REQUESTOR_PRIVATE_KEY) {
    evmRequestorPrivateKey = pulumi.output(process.env.EVM_REQUESTOR_PRIVATE_KEY);
  } else {
    evmRequestorPrivateKey = evmRequestorConfig.getSecret('PRIVATE_KEY');
  }
  const evmRequestorInterval = evmRequestorConfig.get('INTERVAL');
  const evmRequestorInputMaxMCycles = evmRequestorConfig.get('INPUT_MAX_MCYCLES');
  const evmRequestorRampUp = evmRequestorConfig.get('RAMP_UP');
  const evmRequestorLockTimeout = evmRequestorConfig.get('LOCK_TIMEOUT');
  const evmRequestorTimeout = evmRequestorConfig.get('TIMEOUT');
  const evmRequestorSecondsPerMCycle = evmRequestorConfig.get('SECONDS_PER_MCYCLE');
  const evmRequestorRampUpSecondsPerMCycle = evmRequestorConfig.get('RAMP_UP_SECONDS_PER_MCYCLE');
  const evmRequestorExecRateKhz = evmRequestorConfig.get('EXEC_RATE_KHZ');
  const evmRequestorMaxPricePerMCycle = evmRequestorConfig.get('MAX_PRICE_PER_MCYCLE');
  const evmRequestorMaxPriceCap = evmRequestorConfig.get('MAX_PRICE_CAP');

  if (evmRequestorPrivateKey) {
    new OrderGenerator('evm-requestor', {
      chainId,
      stackName,
      autoDeposit: evmRequestorAutoDeposit,
      warnBalanceBelow: evmRequestorWarnBalanceBelow,
      errorBalanceBelow: evmRequestorErrorBalanceBelow,
      privateKey: evmRequestorPrivateKey,
      pinataJWT,
      ethRpcUrl,
      image,
      logLevel,
      setVerifierAddr,
      boundlessMarketAddr,
      ipfsGateway,
      interval: evmRequestorInterval ?? interval,
      lockCollateralRaw,
      rampUp: evmRequestorRampUp,
      inputMaxMCycles: evmRequestorInputMaxMCycles,
      minPricePerMCycle,
      maxPricePerMCycle: evmRequestorMaxPricePerMCycle,
      secondsPerMCycle: evmRequestorSecondsPerMCycle,
      rampUpSecondsPerMCycle: evmRequestorRampUpSecondsPerMCycle,
      vpcId,
      privateSubnetIds,
      boundlessAlertsTopicArns: alertsTopicArns,
      txTimeout,
      lockTimeout: evmRequestorLockTimeout,
      timeout: evmRequestorTimeout,
      execRateKhz: evmRequestorExecRateKhz,
      indexerUrl,
      useZeth: true,
      maxPriceCap: evmRequestorMaxPriceCap,
    });
  }
};
