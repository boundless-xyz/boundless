import { Context, Handler } from "aws-lambda";
import { CloudWatchClient } from "@aws-sdk/client-cloudwatch";
import { getLogGroupName } from "./logGroups";
import { SERVICE_TO_QUERY_STRING_MAPPING } from "./logQueries";
import { encodeCloudWatchLogsInsightsUrl, encodeAwsConsoleUrl } from "./urls";

const LOG_QUERY_MINS_BEFORE = 2;
const LOG_QUERY_MINS_AFTER = 1;

export const getChainName = (chainId: string): string => {
  if (chainId === "11155111") {
    return "Ethereum Sepolia";
  }
  if (chainId === "8453") {
    return "Base Mainnet";
  }
  if (chainId === "84532") {
    return "Base Sepolia";
  }
  return ``;
};

type AlarmEvent = {
  alarmArn: string;
  namespace: string;
  alarmDescription: string;
  timestamp: string;
  metricAlarmName: string;
  metric: string;
  alarmState: string;
}

export const handler: Handler = async (event: AlarmEvent, context: Context): Promise<any> => {
  console.log('Received event', { event, context });

  if (!process.env.SSO_BASE_URL) {
    throw new Error('SSO_BASE_URL is not set');
  }

  const response = await processAlarmEvent(process.env.SSO_BASE_URL, process.env.RUNBOOK_URL, new CloudWatchClient({ region: "us-west-2" }), event);

  return {
    statusCode: 200,
    body: response,
  };
};

export const processAlarmEvent = async (ssoBaseUrl: string, runbookUrl: string, client: CloudWatchClient, event: AlarmEvent): Promise<string> => {
  const { alarmArn, alarmDescription, timestamp, metricAlarmName } = event;

  // Process metric alarm name to get stage, chain id, service name.
  // Supports two formats:
  // 1. <stage>-<chainId>-<service>-<chainId>
  //    e.g. prod-11155111-bento-prover-11155111
  //    e.g. staging-11155111-og-onchain-11155111-log-fatal-SEV2
  // 2. <stage>-<service>-<chainId>-<description>-<severity>
  //    e.g. staging-monitor-11155111-requests_number_from_0xe9669e8fe06aa27d3ed5d85a33453987c80bbdc3-SEV2
  const format1Regex = /^([^-]+)-(\d+)-(.+?)-\2/;
  const format2Regex = /^([^-]+)-(.+?)-(\d+)-.+$/;

  let stage: string;
  let chainId: string;
  let service: string;

  const format1Match = metricAlarmName.match(format1Regex);
  if (format1Match) {
    [, stage, chainId, service] = format1Match;
  } else {
    const format2Match = metricAlarmName.match(format2Regex);
    if (!format2Match) {
      throw new Error(`Unable to parse metric alarm name: ${metricAlarmName} to extract stage, service, and chainId`);
    }
    [, stage, service, chainId] = format2Match;
  }
  console.log(`Parsed stage: ${stage}, chainId: ${chainId}, service: ${service}`);

  const accountId = alarmArn.split(':')[4];
  const region = alarmArn.split(':')[3];
  const chainName = getChainName(chainId);
  const alarmTime = new Date(timestamp);
  const endTime = new Date(alarmTime.getTime() + LOG_QUERY_MINS_AFTER * 60 * 1000);
  const startTime = new Date(alarmTime.getTime() - LOG_QUERY_MINS_BEFORE * 60 * 1000);

  const logGroupName = getLogGroupName(stage, chainId, service);
  const queryString = SERVICE_TO_QUERY_STRING_MAPPING(service, logGroupName, metricAlarmName);

  const logsUrl = await encodeCloudWatchLogsInsightsUrl({
    region,
    logGroupName,
    startTime,
    endTime,
    queryString,
    accountId
  });

  const url = encodeAwsConsoleUrl(ssoBaseUrl, {
    account_id: accountId,
    role_name: 'AWSPowerUserAccess',
  }, {
    destination: logsUrl
  });

  const response = `
<${url}|🔎🔎🔎*View Logs*🔎🔎🔎>

*🏢 Stage:* ${stage}

*⛓️ Chain ID:* ${chainId} ${chainName ? `(${chainName})` : ''}

*🚚 Service:* ${service}

*📝 Description:* ${alarmDescription}

*📅 Time:* ${alarmTime.toISOString()} (UTC)

<${runbookUrl}|📘📘📘*View Runbook*📘📘📘>
`;

  console.log(`Returning: ${response}`);
  return response;
}