import * as pulumi from '@pulumi/pulumi';
import { OrderStreamInstance } from './components/order-stream';
import { getEnvVar } from '../util';

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  
  const ethRpcUrl = isDev ? pulumi.output(getEnvVar("ETH_RPC_URL")) : config.requireSecret('ETH_RPC_URL');
  const rdsPassword = isDev ? pulumi.output(getEnvVar("RDS_PASSWORD")) : config.requireSecret('RDS_PASSWORD');
  const chainId = isDev ? getEnvVar("CHAIN_ID") : config.require('CHAIN_ID');
  
  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const bypassAddrs = config.require('BYPASS_ADDRS');
  const boundlessAddress = config.require('BOUNDLESS_ADDRESS');
  const minBalance = config.require('MIN_BALANCE');
  const baseStackName = config.require('BASE_STACK');
  const orderStreamPingTime = config.requireNumber('ORDER_STREAM_PING_TIME');
  const albDomain = config.getSecret('ALB_DOMAIN');
  const acmCertArn = config.getSecret('ACM_CERT_ARN');

  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
  const pubSubNetIds = baseStack.getOutput('PUBLIC_SUBNET_IDS') as pulumi.Output<string[]>;

  const orderStream = new OrderStreamInstance(`order-stream`, {
    chainId,
    ciCacheSecret,
    dockerDir,
    dockerTag,
    orderStreamPingTime,
    privSubNetIds,
    pubSubNetIds,
    minBalance,
    githubTokenSecret,
    boundlessAddress,
    bypassAddrs,
    vpcId,
    rdsPassword,
    ethRpcUrl,
    albDomain,
    acmCertArn,
  });

  return {
    ORDER_STREAM_LB_URL: orderStream.lbUrl,
    ORDER_STREAM_SWAGGER: orderStream.swaggerUrl,
  };
};
