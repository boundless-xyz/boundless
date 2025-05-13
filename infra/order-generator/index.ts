import * as pulumi from '@pulumi/pulumi';
import { ChainId, getEnvVar } from '../util';
import { SimpleGenerator } from './components/simple-generator';
import { ZethGenerator } from './components/zeth-generator';

require('dotenv').config();

export = () => {
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";

  const baseConfig = new pulumi.Config("order-generator-base");
  const chainId = baseConfig.require('CHAIN_ID');
  const pinataJWT = isDev ? pulumi.output(getEnvVar("PINATA_JWT")) : baseConfig.requireSecret('PINATA_JWT');
  const ethRpcUrl = isDev ? pulumi.output(getEnvVar("ETH_RPC_URL")) : baseConfig.requireSecret('ETH_RPC_URL');
  const orderStreamUrl = isDev 
    ? pulumi.output(getEnvVar("ORDER_STREAM_URL")) 
    : (baseConfig.getSecret('ORDER_STREAM_URL') || pulumi.output(""));
  const githubTokenSecret = baseConfig.getSecret('GH_TOKEN_SECRET');
  const logLevel = baseConfig.require('LOG_LEVEL');
  const dockerDir = baseConfig.require('DOCKER_DIR');
  const dockerTag = baseConfig.require('DOCKER_TAG');
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;
  const setVerifierAddr = baseConfig.require('SET_VERIFIER_ADDR');
  const boundlessMarketAddr = baseConfig.require('BOUNDLESS_MARKET_ADDR');
  const pinataGateway = baseConfig.require('PINATA_GATEWAY_URL');
  const baseStackName = baseConfig.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
  const privateSubnetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
  const boundlessAlertsTopicArn = baseConfig.get('SLACK_ALERTS_TOPIC_ARN');
  
  const simpleConfig = new pulumi.Config("order-generator-simple");
  const simplePrivateKey = isDev ? pulumi.output(getEnvVar("SIMPLE_PRIVATE_KEY")) : simpleConfig.requireSecret('PRIVATE_KEY');
  const simpleInterval = simpleConfig.require('INTERVAL');
  const simpleLockStake = simpleConfig.require('LOCK_STAKE');
  const simpleRampUp = simpleConfig.require('RAMP_UP');
  const simpleMinPricePerMCycle = simpleConfig.require('MIN_PRICE_PER_MCYCLE');
  const simpleMaxPricePerMCycle = simpleConfig.require('MAX_PRICE_PER_MCYCLE');
  
  new SimpleGenerator('order-generator', {
    chainId,
    stackName,
    privateKey: simplePrivateKey,
    pinataJWT,
    ethRpcUrl,
    orderStreamUrl,
    githubTokenSecret,
    logLevel,
    dockerDir,
    dockerTag,
    dockerRemoteBuilder,
    setVerifierAddr,
    boundlessMarketAddr,
    pinataGateway,
    interval: simpleInterval,
    lockStake: simpleLockStake,
    rampUp: simpleRampUp,
    minPricePerMCycle: simpleMinPricePerMCycle,
    maxPricePerMCycle: simpleMaxPricePerMCycle,
    vpcId,
    privateSubnetIds,
    boundlessAlertsTopicArn,
  });

  /* TODO(#630): Re-enable the zeth order generator
  const zethConfig = new pulumi.Config("order-generator-zeth");
  const zethPrivateKey = isDev ? pulumi.output(getEnvVar("ZETH_PRIVATE_KEY")) : zethConfig.requireSecret('PRIVATE_KEY');
  const zethRpcUrl = isDev ? pulumi.output(getEnvVar("ZETH_RPC_URL")) : zethConfig.requireSecret('ZETH_RPC_URL');
  const zethBoundlessRpcUrl = isDev ? pulumi.output(getEnvVar("BOUNDLESS_RPC_URL")) : zethConfig.requireSecret('BOUNDLESS_RPC_URL');
  const zethScheduleMinutes = zethConfig.require('SCHEDULE_MINUTES');
  const zethRetries = zethConfig.require('RETRIES');
  const zethInterval = zethConfig.require('INTERVAL');
  const zethLockStake = zethConfig.require('LOCK_STAKE');
  const zethRampUp = zethConfig.require('RAMP_UP');
  const zethMinPricePerMCycle = zethConfig.require('MIN_PRICE_PER_MCYCLE');
  const zethMaxPricePerMCycle = zethConfig.require('MAX_PRICE_PER_MCYCLE');
  const zethTimeout = zethConfig.require('TIMEOUT');
  const zethLockTimeout = zethConfig.require('LOCK_TIMEOUT');
  new ZethGenerator('order-generator-zeth', {
    chainId,
    stackName,
    privateKey: zethPrivateKey,
    pinataJWT,
    zethRpcUrl,
    boundlessRpcUrl: zethBoundlessRpcUrl,
    orderStreamUrl,
    githubTokenSecret,
    logLevel,
    dockerDir,
    dockerTag,
    dockerRemoteBuilder,
    setVerifierAddr,
    boundlessMarketAddr,
    pinataGateway,
    interval: zethInterval,
    lockStake: zethLockStake,
    rampUp: zethRampUp,
    minPricePerMCycle: zethMinPricePerMCycle,
    maxPricePerMCycle: zethMaxPricePerMCycle,
    vpcId,
    privateSubnetIds,
    boundlessAlertsTopicArn,
    retries: zethRetries,
    scheduleMinutes: zethScheduleMinutes,
    timeout: zethTimeout,
    lockTimeout: zethLockTimeout,
  });
  */
};
