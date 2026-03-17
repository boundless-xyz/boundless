import * as pulumi from '@pulumi/pulumi';
import { OrderStreamInstance } from './components/order-stream';
import { TelemetryInfra } from './components/telemetry';
import { getEnvVar, getServiceNameV1 } from '../util';

require('dotenv').config();

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const isDev = stackName.includes("dev");

  const ethRpcUrl = isDev ? pulumi.output(getEnvVar("ETH_RPC_URL")) : config.requireSecret('ETH_RPC_URL');
  const rdsPassword = isDev ? pulumi.output(getEnvVar("RDS_PASSWORD")) : config.requireSecret('RDS_PASSWORD');

  const chainId = config.require('CHAIN_ID');
  const serviceName = getServiceNameV1(stackName, "order-stream", chainId);

  const githubTokenSecret = config.getSecret('GH_TOKEN_SECRET');
  const dockerDir = config.require('DOCKER_DIR');
  const dockerTag = config.require('DOCKER_TAG');
  const dockerRemoteBuilder = isDev ? process.env.DOCKER_REMOTE_BUILDER : undefined;
  const ciCacheSecret = config.getSecret('CI_CACHE_SECRET');
  const bypassAddrs = config.require('BYPASS_ADDRS');
  const boundlessAddress = config.require('BOUNDLESS_ADDRESS');
  const minBalanceRaw = config.require('MIN_BALANCE_RAW');
  const baseStackName = config.require('BASE_STACK');
  const orderStreamPingTime = config.requireNumber('ORDER_STREAM_PING_TIME');
  const albDomain = config.getSecret('ALB_DOMAIN');
  const boundlessAlertsTopicArn = config.get('SLACK_ALERTS_TOPIC_ARN');
  const boundlessPagerdutyTopicArn = config.get('PAGERDUTY_ALERTS_TOPIC_ARN');
  const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];
  const disableCert = config.getBoolean('DISABLE_CERT') || false;

  const premiumApiKey = config.getSecret('PREMIUM_API_KEY');
  const premiumRateLimit = config.getNumber('PREMIUM_RATE_LIMIT') ?? 10000;
  const standardRateLimit = config.getNumber('STANDARD_RATE_LIMIT') ?? 500;

  const baseStack = new pulumi.StackReference(baseStackName);
  const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
  const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
  const pubSubNetIds = baseStack.getOutput('PUBLIC_SUBNET_IDS') as pulumi.Output<string[]>;

  const redshiftAdminPassword = config.requireSecret('REDSHIFT_ADMIN_PASSWORD');
  const telemetry = new TelemetryInfra(`${serviceName}-telemetry`, {
    serviceName,
    vpcId,
    pubSubNetIds,
    redshiftAdminPassword,
  });

  const orderStream = new OrderStreamInstance(`order-stream`, {
    chainId,
    ciCacheSecret,
    dockerDir,
    dockerTag,
    dockerRemoteBuilder,
    orderStreamPingTime,
    privSubNetIds,
    pubSubNetIds,
    minBalanceRaw,
    githubTokenSecret,
    boundlessAddress,
    bypassAddrs,
    vpcId,
    rdsPassword,
    ethRpcUrl,
    albDomain,
    boundlessAlertsTopicArns: alertsTopicArns,
    disableCert,
    premiumApiKey,
    standardRateLimit,
    premiumRateLimit,
    kinesisHeartbeatStreamName: telemetry.heartbeatStreamName,
    kinesisHeartbeatStreamArn: telemetry.heartbeatStreamArn,
    kinesisEvaluationsStreamName: telemetry.evaluationsStreamName,
    kinesisEvaluationsStreamArn: telemetry.evaluationsStreamArn,
    kinesisCompletionsStreamName: telemetry.completionsStreamName,
    kinesisCompletionsStreamArn: telemetry.completionsStreamArn,
  });

  return {
    ORDER_STREAM_LB_URL: orderStream.lbUrl,
    ORDER_STREAM_SWAGGER: orderStream.swaggerUrl,
    KINESIS_HEARTBEAT_STREAM: telemetry.heartbeatStreamName,
    KINESIS_EVALUATIONS_STREAM: telemetry.evaluationsStreamName,
    KINESIS_COMPLETIONS_STREAM: telemetry.completionsStreamName,
    REDSHIFT_ENDPOINT: telemetry.redshiftEndpoint,
    REDSHIFT_IAM_ROLE_ARN: telemetry.redshiftIamRoleArn,
  };
};
