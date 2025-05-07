import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { getEnvVar, ChainId, getServiceNameV1, Severity } from "../util";
import { BentoBroker } from "./components/bentoBroker";
import { BonsaiBroker } from "./components/bonsaiBroker";
require('dotenv').config();

export = () => {
  // Read config
  const config = new pulumi.Config();

  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;

  const privateKey = isDev ? getEnvVar("PRIVATE_KEY") : config.requireSecret('PRIVATE_KEY');
  const bentoProverPrivateKey = isDev ? getEnvVar("BENTO_PROVER_PRIVATE_KEY") : config.requireSecret('BENTO_PROVER_PRIVATE_KEY');
  const bentoProverSshPublicKey = isDev ? getEnvVar("BENTO_PROVER_SSH_PUBLIC_KEY") : config.requireSecret('BENTO_PROVER_SSH_PUBLIC_KEY');
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : config.requireSecret('ETH_RPC_URL');
  const orderStreamUrl = isDev ? getEnvVar("ORDER_STREAM_URL") : config.requireSecret('ORDER_STREAM_URL');

  const baseStackName = config.require('BASE_STACK');
  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID');
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');
  const pubSubNetIds = baseStack.getOutput('PUBLIC_SUBNET_IDS');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');

  const setVerifierAddr = config.require('SET_VERIFIER_ADDR');
  const proofMarketAddr = config.require('PROOF_MARKET_ADDR');
  
  const bonsaiApiUrl = config.require('BONSAI_API_URL');
  const bonsaiApiKey = isDev ? getEnvVar("BONSAI_API_KEY") : config.getSecret('BONSAI_API_KEY');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  
  const brokerTomlPath = config.require('BROKER_TOML_PATH')
  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');

  const bentoBrokerServiceName = getServiceNameV1(stackName, "bento-proverx", ChainId.SEPOLIA);
  const bentoBroker = new BentoBroker(bentoBrokerServiceName, {
    chainId: ChainId.SEPOLIA,
    ethRpcUrl,
    gitBranch: "main",
    privateKey: bentoProverPrivateKey,
    baseStackName,
    orderStreamUrl,
    brokerTomlPath,
    boundlessMarketAddr: proofMarketAddr,
    setVerifierAddr,
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
  // const bonsaiBroker = new BonsaiBroker(bonsaiBrokerServiceName, {
  //   chainId: ChainId.SEPOLIA,
  //   ethRpcUrl,
  //   privateKey,
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
  }
};
