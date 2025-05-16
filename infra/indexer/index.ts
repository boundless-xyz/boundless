import * as pulumi from '@pulumi/pulumi';
import { IndexerInstance } from './components/indexer';
import { MonitorLambda } from './components/monitor-lambda';
import { getEnvVar } from '../util';

require('dotenv').config();

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;

  const ethRpcUrl = isDev ? pulumi.output(getEnvVar("ETH_RPC_URL")) : config.requireSecret('ETH_RPC_URL');
  const rdsPassword = isDev ? pulumi.output(getEnvVar("RDS_PASSWORD")) : config.requireSecret('RDS_PASSWORD');
  const chainId = isDev ? getEnvVar("CHAIN_ID") : config.require('CHAIN_ID');

  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const boundlessAddress = config.require('BOUNDLESS_ADDRESS');
  const baseStackName = config.require('BASE_STACK');
  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');
  const startBlock = config.require('START_BLOCK');
  const rustLogLevel = config.get('RUST_LOG_LEVEL') || 'info';

  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
  const pubSubNetIds = baseStack.getOutput('PUBLIC_SUBNET_IDS') as pulumi.Output<string[]>;

  const indexer = new IndexerInstance(`indexer`, {
    chainId,
    ciCacheSecret,
    dockerDir,
    dockerTag,
    privSubNetIds,
    pubSubNetIds,
    githubTokenSecret,
    boundlessAddress,
    vpcId,
    rdsPassword,
    ethRpcUrl,
    boundlessAlertsTopicArn,
    startBlock,
    dockerRemoteBuilder,
  });

  new MonitorLambda('monitor', {
    vpcId: vpcId,
    privSubNetIds: privSubNetIds,
    intervalMinutes: '1',
    dbUrlSecret: indexer.dbUrlSecret,
    rdsSgId: indexer.rdsSecurityGroupId,
    chainId: chainId,
    rustLogLevel: rustLogLevel,
    boundlessAlertsTopicArn: boundlessAlertsTopicArn,
  }, { parent: indexer, dependsOn: indexer });

};
