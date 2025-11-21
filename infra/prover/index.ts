import * as pulumi from '@pulumi/pulumi';
import { getEnvVar, ChainId, getServiceNameV1, getChainId } from "../util";
import { BentoEC2Broker } from "./components/bentoBroker";
require('dotenv').config();

export = () => {
  // Read config
  const baseConfig = new pulumi.Config("base-prover");
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
  const chainId = baseConfig.require('CHAIN_ID');
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;
  const ethRpcUrl = isDev ? getEnvVar("ETH_RPC_URL") : baseConfig.requireSecret('ETH_RPC_URL');
  const ethRpcUrls = isDev ? getEnvVar("ETH_RPC_URLS") : baseConfig.requireSecret('ETH_RPC_URLS');
  const orderStreamUrl = isDev ? getEnvVar("ORDER_STREAM_URL") : baseConfig.requireSecret('ORDER_STREAM_URL');
  const dockerDir = baseConfig.require('DOCKER_DIR');
  const dockerTag = baseConfig.require('DOCKER_TAG');
  const setVerifierAddr = baseConfig.require('SET_VERIFIER_ADDR');
  const boundlessMarketAddr = baseConfig.require('BOUNDLESS_MARKET_ADDR');
  const collateralTokenAddress = baseConfig.require('COLLATERAL_TOKEN_ADDRESS');
  const ciCacheSecret = baseConfig.getSecret('CI_CACHE_SECRET');
  const githubTokenSecret = baseConfig.getSecret('GH_TOKEN_SECRET');
  const boundlessAlertsTopicArn = baseConfig.get('SLACK_ALERTS_TOPIC_ARN');
  const boundlessPagerdutyTopicArn = baseConfig.get('PAGERDUTY_ALERTS_TOPIC_ARN');
  const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];


  // Bento Prover Config
  const bentoProverBranch = bentoConfig.require('BRANCH');
  const bentoProverSshPublicKey = isDev ? process.env.BENTO_PROVER_SSH_PUBLIC_KEY : bentoConfig.getSecret('SSH_PUBLIC_KEY');
  const bentoProverPrivateKey1 = isDev ? getEnvVar("BENTO_PROVER_PRIVATE_KEY_1") : bentoConfig.requireSecret('PRIVATE_KEY_1');
  const bentoProverPrivateKey2 = isDev ? getEnvVar("BENTO_PROVER_PRIVATE_KEY_2") : bentoConfig.requireSecret('PRIVATE_KEY_2');
  const prover1PovwLogId = bentoConfig.requireSecret('POVW_LOG_ID_1');
  const prover2PovwLogId = bentoConfig.requireSecret('POVW_LOG_ID_2');
  const segmentSize = bentoConfig.requireNumber('SEGMENT_SIZE');
  const snarkTimeout = bentoConfig.requireNumber('SNARK_TIMEOUT');
  const logJson = bentoConfig.getBoolean('LOG_JSON');
  const bentoBrokerTomlPath = bentoConfig.require('BROKER_TOML_PATH')

  const bentoBroker1ServiceName = getServiceNameV1(stackName, "bento-prover-1", chainId);
  const bentoBroker2ServiceName = getServiceNameV1(stackName, "bento-prover-2", chainId);
  let bentoBroker1: BentoEC2Broker | undefined;
  let bentoBroker2: BentoEC2Broker | undefined;

  // For instance 1, we use a single RPC URL
  // For instance 2, we use multiple RPC URLs
  // This allows us to compare the reliability of the two configurations
  let bentoBroker1RpcUrls: string | pulumi.Output<string> = ethRpcUrl;
  let bentoBroker2RpcUrls: string | pulumi.Output<string> = ethRpcUrls;

  if (process.env.SKIP_BENTO !== "true") {
    bentoBroker1 = new BentoEC2Broker(bentoBroker1ServiceName, {
      chainId: getChainId(chainId),
      collateralTokenAddress,
      ethRpcUrls: bentoBroker1RpcUrls,
      gitBranch: bentoProverBranch,
      privateKey: bentoProverPrivateKey1,
      baseStackName,
      orderStreamUrl,
      brokerTomlPath: bentoBrokerTomlPath,
      boundlessMarketAddress: boundlessMarketAddr,
      setVerifierAddress: setVerifierAddr,
      segmentSize,
      snarkTimeout,
      vpcId,
      pubSubNetIds,
      dockerDir,
      dockerTag,
      ciCacheSecret,
      githubTokenSecret,
      boundlessAlertsTopicArns: alertsTopicArns,
      sshPublicKey: bentoProverSshPublicKey,
      logJson,
      povwLogId: prover1PovwLogId,
    });

    bentoBroker2 = new BentoEC2Broker(bentoBroker2ServiceName, {
      chainId: getChainId(chainId),
      collateralTokenAddress,
      ethRpcUrls: bentoBroker2RpcUrls,
      gitBranch: bentoProverBranch,
      privateKey: bentoProverPrivateKey2,
      baseStackName,
      orderStreamUrl,
      brokerTomlPath: bentoBrokerTomlPath,
      boundlessMarketAddress: boundlessMarketAddr,
      setVerifierAddress: setVerifierAddr,
      segmentSize,
      snarkTimeout,
      vpcId,
      pubSubNetIds,
      dockerDir,
      dockerTag,
      ciCacheSecret,
      githubTokenSecret,
      boundlessAlertsTopicArns: alertsTopicArns,
      sshPublicKey: bentoProverSshPublicKey,
      logJson,
      povwLogId: prover2PovwLogId,
    });
  }

  return {
    bentoBroker1PublicIp: bentoBroker1?.instance.publicIp ?? undefined,
    bentoBroker1PublicDns: bentoBroker1?.instance.publicDns ?? undefined,
    bentoBroker1InstanceId: bentoBroker1?.instance.id ?? undefined,
    bentoBroker1UpdateCommandArn: bentoBroker1?.updateCommandArn ?? undefined,
    bentoBroker1UpdateCommandId: bentoBroker1?.updateCommandId ?? undefined,
    bentoBroker2PublicIp: bentoBroker2?.instance.publicIp ?? undefined,
    bentoBroker2PublicDns: bentoBroker2?.instance.publicDns ?? undefined,
    bentoBroker2InstanceId: bentoBroker2?.instance.id ?? undefined,
    bentoBroker2UpdateCommandArn: bentoBroker2?.updateCommandArn ?? undefined,
    bentoBroker2UpdateCommandId: bentoBroker2?.updateCommandId ?? undefined,
  }
};
