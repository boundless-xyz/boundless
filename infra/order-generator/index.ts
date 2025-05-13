import * as pulumi from '@pulumi/pulumi';
import { ChainId, getEnvVar } from '../util';
import { OrderGenerator } from './components/order-generator';

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
  const interval = baseConfig.require('INTERVAL');
  const lockStake = baseConfig.require('LOCK_STAKE');
  const rampUp = baseConfig.require('RAMP_UP');
  const minPricePerMCycle = baseConfig.require('MIN_PRICE_PER_MCYCLE');
  const maxPricePerMCycle = baseConfig.require('MAX_PRICE_PER_MCYCLE');
  const secondsPerMCycle = baseConfig.require('SECONDS_PER_MCYCLE');
  
  const offchainConfig = new pulumi.Config("order-generator-offchain");
  const offchainPrivateKey = isDev ? pulumi.output(getEnvVar("OFFCHAIN_PRIVATE_KEY")) : offchainConfig.requireSecret('PRIVATE_KEY');
  new OrderGenerator('offchain', {
    chainId,
    stackName,
    privateKey: offchainPrivateKey,
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
    interval,
    lockStake,
    rampUp,
    minPricePerMCycle,
    maxPricePerMCycle,
    secondsPerMCycle,
    vpcId,
    privateSubnetIds,
    boundlessAlertsTopicArn,
  });

  const onchainConfig = new pulumi.Config("order-generator-onchain");
  const onchainPrivateKey = isDev ? pulumi.output(getEnvVar("ONCHAIN_PRIVATE_KEY")) : onchainConfig.requireSecret('PRIVATE_KEY');
  new OrderGenerator('onchain', {
    chainId,
    stackName,
    privateKey: onchainPrivateKey,
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
    interval,
    lockStake,
    rampUp,
    minPricePerMCycle,
    maxPricePerMCycle,
    secondsPerMCycle,
    vpcId,
    privateSubnetIds,
    boundlessAlertsTopicArn,
  });

};
