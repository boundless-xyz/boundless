import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { getEnvVar, ChainId, getServiceNameV1, Severity } from "../util";
import { BentoEC2Broker } from "./components/bentoBroker";
import { BonsaiECSBroker } from "./components/bonsaiBroker";
require('dotenv').config();

export = () => {
  // Read config
  const config = new pulumi.Config();

  const baseConfig = new pulumi.Config("base-prover");
  const bonsaiConfig = new pulumi.Config("bonsai-prover");
  const bentoConfig = new pulumi.Config("bento-prover");

  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";

  // Pulumi shared outputs from the bootstrap stack
  const baseStackName = baseConfig.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');
  const pubSubNetIds = baseStack.getOutput('PUBLIC_SUBNET_IDS');
  
  // Base Shared Prover Config
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : baseConfig.requireSecret('ETH_RPC_URL');
  const orderStreamUrl = isDev ? getEnvVar("ORDER_STREAM_URL") : baseConfig.requireSecret('ORDER_STREAM_URL');
  const dockerDir = baseConfig.require('DOCKER_DIR');
  const dockerTag = baseConfig.require('DOCKER_TAG');
  const setVerifierAddr = baseConfig.require('SET_VERIFIER_ADDR');
  const boundlessMarketAddr = baseConfig.require('BOUNDLESS_MARKET_ADDR');
  const ciCacheSecret = baseConfig.getSecret('CI_CACHE_SECRET');
  const githubTokenSecret = baseConfig.getSecret('GH_TOKEN_SECRET');
  const brokerTomlPath = baseConfig.require('BROKER_TOML_PATH')
  const boundlessAlertsTopicArn = baseConfig.get('SLACK_ALERTS_TOPIC_ARN');

  // Bonsai Prover Config
  const bonsaiProverPrivateKey = isDev ? getEnvVar("BONSAI_PROVER_PRIVATE_KEY") : bonsaiConfig.requireSecret('PRIVATE_KEY');
  const bonsaiApiUrl = bonsaiConfig.require('BONSAI_API_URL');
  const bonsaiApiKey = isDev ? getEnvVar("BONSAI_API_KEY") : bonsaiConfig.getSecret('BONSAI_API_KEY');

  // Bento Prover Config
  const bentoProverSshPublicKey = isDev ? process.env.BENTO_PROVER_SSH_PUBLIC_KEY : bentoConfig.getSecret('SSH_PUBLIC_KEY');
  const bentoProverPrivateKey = isDev ? getEnvVar("BENTO_PROVER_PRIVATE_KEY") : bentoConfig.requireSecret('PRIVATE_KEY');
  const segmentSize = bentoConfig.requireNumber('SEGMENT_SIZE');

  const bentoBrokerServiceName = getServiceNameV1(stackName, "bento-proverx", ChainId.SEPOLIA);
  const bentoBroker = new BentoEC2Broker(bentoBrokerServiceName, {
    chainId: ChainId.SEPOLIA,
    ethRpcUrl,
    gitBranch: "main",
    privateKey: bentoProverPrivateKey,
    baseStackName,
    orderStreamUrl,
    brokerTomlPath,
    boundlessMarketAddress: boundlessMarketAddr,
    setVerifierAddress: setVerifierAddr,
    segmentSize,
    vpcId,
    pubSubNetIds,
    dockerDir,
    dockerTag,
    ciCacheSecret,
    githubTokenSecret,
    boundlessAlertsTopicArn,
    sshPublicKey: bentoProverSshPublicKey
  });

  // const bonsaiBrokerServiceName = getServiceNameV1(stackName, "bonsai-prover", ChainId.SEPOLIA);
  // const bonsaiBroker = new BonsaiECSBroker(bonsaiBrokerServiceName, {
  //   chainId: ChainId.SEPOLIA,
  //   ethRpcUrl,
  //   privateKey: bonsaiProverPrivateKey,
  //   baseStackName,
  //   bonsaiApiUrl,
  //   bonsaiApiKey,
  //   orderStreamUrl,
  //   brokerTomlPath,
  //   proofMarketAddr,
  //   setVerifierAddr,
  //   vpcId,
  //   privSubNetIds,
  //   dockerDir,
  //   dockerTag,
  //   ciCacheSecret,
  //   githubTokenSecret,
  //   boundlessAlertsTopicArn,
  //   dockerRemoteBuilder,
  // });

  return {
    bentoBrokerPublicIp: bentoBroker.instance.publicIp,
    bentoBrokerPublicDns: bentoBroker.instance.publicDns,
    bentoBrokerInstanceId: bentoBroker.instance.id,
    bentoBrokerUpdateCommandArn: bentoBroker.updateCommandArn,
    bentoBrokerUpdateCommandId: bentoBroker.updateCommandId,
  }
};
