import { Context, Handler } from "aws-lambda";
import { CloudWatchClient, ListTagsForResourceCommand } from "@aws-sdk/client-cloudwatch";
import { STSClient, AssumeRoleCommand } from "@aws-sdk/client-sts";
import { getLogGroupName } from "./logGroups";
import { SERVICE_TO_QUERY_STRING_MAPPING } from "./logQueries";
import { encodeCloudWatchLogsInsightsUrl, encodeAwsConsoleUrl } from "./urls";
import { accountIdToRoleName } from "./roles";

const LOG_QUERY_MINS_BEFORE = 2;
const LOG_QUERY_MINS_AFTER = 1;
const RUNBOOK_BASE_URL = "https://github.com/boundless-xyz/runbooks";

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

  const response = await processAlarmEvent(process.env.SSO_BASE_URL, new CloudWatchClient({ region: "us-west-2" }), event);

  return {
    statusCode: 200,
    body: response,
  };
};

interface AlarmTags {
  [key:string]: string;
}

async function queryAlarmTags(alarmArn: string, accountId: string, region: string): Promise<AlarmTags> {
  const stsClient = new STSClient({
    region: region
  });
  const stsResponse = await stsClient.send(new AssumeRoleCommand({
    RoleArn: `arn:aws:iam::${accountId}:role/alarmListTagsRole`,
    RoleSessionName: "Log-Lambda-list-alarm-tags"
  }));

  const cwClient = new CloudWatchClient({
    region: region,
    credentials: {
      accessKeyId: stsResponse.Credentials.AccessKeyId,
      secretAccessKey: stsResponse.Credentials.SecretAccessKey,
      sessionToken: stsResponse.Credentials.SessionToken,
    }
  });
  let tags: AlarmTags = {}
  const tagsResponse = await cwClient.send(new ListTagsForResourceCommand({
    ResourceARN: alarmArn,
  }));

  tagsResponse.Tags.forEach( (tag) => {
    tags[tag.Key] = tag.Value;
  });

  return tags
}

/**
 * Extract a runbook file slug from the alarm name or description.
 * Returns a slug like "b-sub-001" or "bento-down" that maps to a .md file
 * in the runbooks GitHub repo (e.g. https://github.com/boundless-xyz/runbooks/blob/main/b-sub-001.md).
 */
function getRunbookSlug(metricAlarmName: string, alarmDescription: string): string {
  // Try to extract broker error code like [B-SUB-001] from description
  const codeMatch = alarmDescription.match(/\[B-[A-Z]+-\w+\]/);
  if (codeMatch) {
    return codeMatch[0].replace(/[\[\]]/g, '').toLowerCase();
  }

  // Try to extract from alarm name patterns
  const alarmLower = metricAlarmName.toLowerCase();
  if (alarmLower.includes('bento-down') || alarmLower.includes('bento-active')) return 'bento-down';
  if (alarmLower.includes('no-containers') || alarmLower.includes('bento-containers')) return 'no-containers';
  if (alarmLower.includes('memory')) return 'memory-high';
  if (alarmLower.includes('disk')) return 'disk-high';

  return '';
}

export const processAlarmEvent = async (ssoBaseUrl: string, client: CloudWatchClient, event: AlarmEvent): Promise<string> => {
  const { alarmArn, alarmDescription, timestamp, metricAlarmName } = event;

  // Process metric alarm name to get stage, chain id, service name.
  // Supports two formats:
  // 1. l-<stage>-<chainId>-<service>-<chainId>-<additional-parts>
  //    e.g. l-prod-84532-og-offchain-84532-log-err-SEV1
  // 2. l-<stage>-<service>-<chainId>-<description>-<severity>
  //    e.g. l-staging-monitor-11155111-requests_number_from_0xe9669e8fe06aa27d3ed5d85a33453987c80bbdc3-SEV2
  const format1Regex = /^(?:l-)?([^-]+)-(\d+)-(.+?)-\2/;
  const format2Regex = /^(?:l-)?([^-]+)-(.+?)-(\d+)-.+$/;

  let stage: string;
  let chainId: string;
  let service: string;
  let logGroupName: string;

  const accountId = alarmArn.split(':')[4];
  const region = alarmArn.split(':')[3];

  // Attempt to retrieve the needed information from alarm tags
  let tagsResponse: AlarmTags | undefined;
  try {
    tagsResponse = await queryAlarmTags(alarmArn, accountId, region);
  } catch (e) {
    console.log(`Failed to query alarm tags: ${e}`);
  }
  if (tagsResponse) {
    stage = tagsResponse["StackName"]
    chainId = tagsResponse["ChainId"]
    service = tagsResponse["ServiceName"]
    logGroupName = tagsResponse["LogGroupName"]
    console.log(`Retrieved from tags stage: ${stage}, chainId: ${chainId}, service: ${service}, logGroupName: ${logGroupName}`)
  }

  // Attempt to parse the needed information from the alarm name if any variables are still undefined
  if (stage === undefined || chainId === undefined || service === undefined) {
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
  }

  const chainName = getChainName(chainId);
  const alarmTime = new Date(timestamp);
  const endTime = new Date(alarmTime.getTime() + LOG_QUERY_MINS_AFTER * 60 * 1000);
  const startTime = new Date(alarmTime.getTime() - LOG_QUERY_MINS_BEFORE * 60 * 1000);

  // Map to a log group name if we haven't already found one
  if (logGroupName === undefined) {
    logGroupName = getLogGroupName(stage, chainId, service);
  }
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
    role_name: accountIdToRoleName(accountId),
  }, {
    destination: logsUrl
  });

  const runbookSlug = getRunbookSlug(metricAlarmName, alarmDescription);
  const fullRunbookUrl = runbookSlug ? `${RUNBOOK_BASE_URL}/blob/main/${runbookSlug}.md` : RUNBOOK_BASE_URL;

  const response = `
<${url}|🔎🔎🔎*View Logs*🔎🔎🔎>

*🏢 Stage:* ${stage}

*⛓️ Chain ID:* ${chainId} ${chainName ? `(${chainName})` : ''}

*🚚 Service:* ${service}

*📝 Description:* ${alarmDescription}

*📅 Time:* ${alarmTime.toISOString()} (UTC)

<${fullRunbookUrl}|📘📘📘*View Runbook*📘📘📘>
`;

  console.log(`Returning: ${response}`);
  return response;
}