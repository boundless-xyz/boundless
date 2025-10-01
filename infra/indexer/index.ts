import * as pulumi from '@pulumi/pulumi';
import { IndexerShared } from './components/indexer-infra';
import { MarketIndexer } from './components/market-indexer';
import { RewardsIndexer } from './components/rewards-indexer';
import { MonitorLambda } from './components/monitor-lambda';
import { IndexerApi } from './components/indexer-api';
import { getEnvVar, getServiceNameV1 } from '../util';

require('dotenv').config();

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;

  const ethRpcUrl = isDev ? pulumi.output(getEnvVar("ETH_RPC_URL")) : config.requireSecret('ETH_RPC_URL');
  const rdsPassword = isDev ? pulumi.output(getEnvVar("RDS_PASSWORD")) : config.requireSecret('RDS_PASSWORD');
  const chainId = config.require('CHAIN_ID');

  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const baseStackName = config.require('BASE_STACK');
  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');
  const boundlessPagerdutyTopicArn = config.get('PAGERDUTY_ALERTS_TOPIC_ARN');
  const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];
  const rustLogLevel = config.get('RUST_LOG') || 'info';

  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
  const indexerServiceName = getServiceNameV1(stackName, "indexer", chainId);
  const monitorServiceName = getServiceNameV1(stackName, "monitor", chainId);
  const apiServiceName = getServiceNameV1(stackName, "api", chainId);

  // Metric namespace for service metrics, e.g. operation health of the monitor/indexer infra
  const serviceMetricsNamespace = `Boundless/Services/${indexerServiceName}`;
  const marketName = getServiceNameV1(stackName, "", chainId);
  // Metric namespace for market metrics, e.g. fulfillment success rate, order count, etc.
  const marketMetricsNamespace = `Boundless/Market/${marketName}`;

  const boundlessAddress = config.get('BOUNDLESS_ADDRESS');
  const startBlock = boundlessAddress ? config.require('START_BLOCK') : undefined;

  const vezkcAddress = config.get('VEZKC_ADDRESS');
  const zkcAddress = config.get('ZKC_ADDRESS');
  const povwAccountingAddress = config.get('POVW_ACCOUNTING_ADDRESS');
  const indexerApiDomain = config.get('INDEXER_API_DOMAIN');

  const shouldDeployMarket = !!boundlessAddress && !!startBlock;
  const shouldDeployRewards = !!vezkcAddress && !!zkcAddress && !!povwAccountingAddress;

  if (!shouldDeployMarket && !shouldDeployRewards) {
    return {};
  }

  const infra = new IndexerShared(indexerServiceName, {
    serviceName: indexerServiceName,
    vpcId,
    privSubNetIds,
    rdsPassword,
  });

  let marketIndexer: MarketIndexer | undefined;
  if (shouldDeployMarket && boundlessAddress && startBlock) {
    marketIndexer = new MarketIndexer(indexerServiceName, {
      infra,
      privSubNetIds,
      ciCacheSecret,
      githubTokenSecret,
      dockerDir,
      dockerTag,
      boundlessAddress,
      ethRpcUrl,
      startBlock,
      serviceMetricsNamespace,
      boundlessAlertsTopicArns: alertsTopicArns,
      dockerRemoteBuilder,
    }, { parent: infra });
  }

  let rewardsIndexer: RewardsIndexer | undefined;
  if (shouldDeployRewards && vezkcAddress && zkcAddress && povwAccountingAddress) {
    rewardsIndexer = new RewardsIndexer(indexerServiceName, {
      infra,
      privSubNetIds,
      ciCacheSecret,
      githubTokenSecret,
      dockerDir,
      dockerTag,
      ethRpcUrl,
      vezkcAddress,
      zkcAddress,
      povwAccountingAddress,
      serviceMetricsNamespace,
      boundlessAlertsTopicArns: alertsTopicArns,
      dockerRemoteBuilder,
    }, { parent: infra });
  }

  const sharedDependencies: pulumi.Resource[] = [infra.dbUrlSecret, infra.dbUrlSecretVersion];
  if (marketIndexer) {
    sharedDependencies.push(marketIndexer);
  }
  if (rewardsIndexer) {
    sharedDependencies.push(rewardsIndexer);
  }

  if (shouldDeployMarket && marketIndexer) {
    new MonitorLambda(monitorServiceName, {
      vpcId: vpcId,
      privSubNetIds: privSubNetIds,
      intervalMinutes: '1',
      dbUrlSecret: infra.dbUrlSecret,
      rdsSgId: infra.rdsSecurityGroupId,
      chainId: chainId,
      rustLogLevel: rustLogLevel,
      boundlessAlertsTopicArns: alertsTopicArns,
      serviceMetricsNamespace,
      marketMetricsNamespace,
    }, { parent: infra, dependsOn: sharedDependencies });
  }


  let api: IndexerApi | undefined;
  if (shouldDeployRewards && rewardsIndexer) {
    api = new IndexerApi(apiServiceName, {
      vpcId: vpcId,
      privSubNetIds: privSubNetIds,
      dbUrlSecret: infra.dbUrlSecret,
      rdsSgId: infra.rdsSecurityGroupId,
      indexerSgId: infra.indexerSecurityGroup.id,
      rustLogLevel: rustLogLevel,
      domain: indexerApiDomain,
    }, { parent: infra, dependsOn: sharedDependencies });
  }

  return api
    ? {
      apiEndpoint: api.cloudFrontDomain,
      apiGatewayEndpoint: api.apiEndpoint,
      distributionId: api.distributionId,
    }
    : {};

};
