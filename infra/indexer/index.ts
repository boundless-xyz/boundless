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
  const chainId = config.require('CHAIN_ID');

  const ethRpcUrl = isDev ? pulumi.output(getEnvVar("ETH_RPC_URL")) : config.requireSecret('ETH_RPC_URL');
  const rdsPassword = isDev ? pulumi.output(getEnvVar("RDS_PASSWORD")) : config.requireSecret('RDS_PASSWORD');

  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  const dockerDir = config.get('DOCKER_DIR') || '../../';
  const dockerTag = config.get('DOCKER_TAG') || 'latest';
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const baseStackName = config.require('BASE_STACK');
  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');
  const boundlessPagerdutyTopicArn = config.get('PAGERDUTY_ALERTS_TOPIC_ARN');
  const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];

  const rustLogApi = config.get('RUST_LOG_API') || 'info';
  const rustLogMonitor = config.get('RUST_LOG_MONITOR') || 'info';
  const rustLogIndexer = config.get('RUST_LOG_INDEXER') || 'info';

  // Proxy-authenticated client-IP rate limiting (CloudFront WAF)
  const proxySecret = isDev ? pulumi.output(process.env.PROXY_SECRET ?? "none") : config.requireSecret('PROXY_SECRET');

  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
  const indexerServiceName = getServiceNameV1(stackName, "indexer", chainId);
  const monitorServiceName = getServiceNameV1(stackName, "monitor", chainId);
  const indexerApiServiceName = getServiceNameV1(stackName, "indexer-api", chainId);

  // Metric namespace for service metrics, e.g. operation health of the monitor/indexer infra
  const serviceMetricsNamespace = `Boundless/Services/${indexerServiceName}`;
  const marketName = getServiceNameV1(stackName, "", chainId);
  // Metric namespace for market metrics, e.g. fulfillment success rate, order count, etc.
  const marketMetricsNamespace = `Boundless/Market/${marketName}`;

  const boundlessAddress = config.get('BOUNDLESS_ADDRESS');
  const startBlock = boundlessAddress ? config.get('START_BLOCK') || '35060420' : undefined;

  const vezkcAddress = config.get('VEZKC_ADDRESS');
  const zkcAddress = config.get('ZKC_ADDRESS');
  const povwAccountingAddress = config.get('POVW_ACCOUNTING_ADDRESS');
  const indexerApiDomain = config.get('INDEXER_API_DOMAIN');
  const allowedIpAddresses = config.getSecret('ALLOWED_IP_ADDRESSES');

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
    const logsEthRpcUrl = isDev ? pulumi.output(getEnvVar("LOGS_ETH_RPC_URL")) : config.requireSecret('LOGS_ETH_RPC_URL');
    const orderStreamApiKey = isDev ? pulumi.output(getEnvVar("ORDER_STREAM_API_KEY")) : config.requireSecret('ORDER_STREAM_API_KEY');
    const orderStreamUrl = isDev ? pulumi.output(getEnvVar("ORDER_STREAM_URL")) : config.getSecret('ORDER_STREAM_URL');
    const bentoApiUrl = isDev ? pulumi.output(process.env.BENTO_API_URL || '') : config.getSecret('BENTO_API_URL');
    const bentoApiKey = isDev ? pulumi.output(process.env.BENTO_API_KEY || '') : config.getSecret('BENTO_API_KEY');

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
      orderStreamUrl,
      orderStreamApiKey,
      logsEthRpcUrl,
      bentoApiUrl,
      bentoApiKey,
      rustLogLevel: rustLogIndexer,
    }, { parent: infra, dependsOn: [infra, infra.cacheBucket, infra.dbUrlSecret, infra.dbUrlSecretVersion, infra.dbReaderUrlSecret, infra.dbReaderUrlSecretVersion] });
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
    }, { parent: infra, dependsOn: [infra, infra.dbUrlSecret, infra.dbUrlSecretVersion] });
  }

  const sharedDependencies: pulumi.Resource[] = [infra.dbUrlSecret, infra.dbUrlSecretVersion, infra.dbReaderUrlSecret, infra.dbReaderUrlSecretVersion];
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
      indexerSgId: infra.indexerSecurityGroup.id,
      chainId: chainId,
      rustLogLevel: rustLogMonitor,
      boundlessAlertsTopicArns: alertsTopicArns,
      serviceMetricsNamespace,
      marketMetricsNamespace,
    }, { parent: infra, dependsOn: sharedDependencies });
  }

  let api: IndexerApi | undefined;
  if (shouldDeployMarket && marketIndexer || shouldDeployRewards && rewardsIndexer) {
    api = new IndexerApi(indexerApiServiceName, {
      vpcId: vpcId,
      privSubNetIds: privSubNetIds,
      dbReaderUrlSecret: infra.dbReaderUrlSecret,
      secretHash: infra.secretHash,
      rdsSgId: infra.rdsSecurityGroupId,
      indexerSgId: infra.indexerSecurityGroup.id,
      rustLogLevel: rustLogApi,
      chainId: chainId,
      domain: indexerApiDomain,
      boundlessAlertsTopicArns: alertsTopicArns,
      databaseVersion: infra.databaseVersion,
      proxySecret,
      allowedIpAddresses,
    }, { parent: infra, dependsOn: sharedDependencies });
  }

  const outputs: Record<string, any> = {};

  if (api) {
    outputs.apiEndpoint = api.cloudFrontDomain;
    outputs.apiGatewayEndpoint = api.apiEndpoint;
    outputs.distributionId = api.distributionId;
  }

  if (marketIndexer) {
    outputs.backfillLambdaName = marketIndexer.backfillLambdaName;
  }

  return outputs;

};
