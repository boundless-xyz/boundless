import { Context, Handler } from "aws-lambda";
import { getChainName } from "../../../util";
import { CloudWatchClient } from "@aws-sdk/client-cloudwatch";
import { SERVICE_TO_LOG_GROUP_NAME } from "./logGroups";
import { SERVICE_TO_QUERY_STRING_MAPPING } from "./logQueries";
import { encodeCloudWatchLogsInsightsUrl, encodeAwsConsoleUrl } from "./urls";

const LOG_QUERY_MINS_BEFORE = 1;
const LOG_QUERY_MINS_AFTER = 1;

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
  console.log(JSON.stringify(event));
  console.log(JSON.stringify(context));

  if (!process.env.SSO_BASE_URL) {
    throw new Error('SSO_BASE_URL is not set');
  }

  const response = await processAlarmEvent(process.env.SSO_BASE_URL, new CloudWatchClient({ region: "us-west-2" }), event);

  return {
    statusCode: 200,
    body: response,
  };
};

export const processAlarmEvent = async (ssoBaseUrl: string, client: CloudWatchClient, event: AlarmEvent): Promise<string> => {
  const { alarmArn, alarmDescription, timestamp, metricAlarmName } = event;

  // Process metric alarm name to get stage, chain id, service name.
  // Assumes format: <stage>-<chainId>-<service>-<chainId>
  // e.g. prod-11155111-bento-prover-11155111
  const regex = /^([^-]+)-(\d+)-(.+?)-\2-/;
  const match = metricAlarmName.match(regex);
  if (!match) {
    throw new Error(`Invalid metric alarm name format: ${metricAlarmName}`);
  }

  const [, stage, chainId, service] = match;
  const accountId = alarmArn.split(':')[4];
  const region = alarmArn.split(':')[3];
  const chainName = getChainName(chainId);
  const alarmTime = new Date(timestamp);
  const endTime = new Date(alarmTime.getTime() + LOG_QUERY_MINS_AFTER * 60 * 1000);
  const startTime = new Date(alarmTime.getTime() - LOG_QUERY_MINS_BEFORE * 60 * 1000);

  const logGroupName = SERVICE_TO_LOG_GROUP_NAME(stage, chainId, service);
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

  return `
*Stage:* ${stage}
*Chain ID:* ${chainName} (${chainId})
*Service:* ${service}
*Alarm Description:* ${alarmDescription}
*Alarm Time:* ${alarmTime.toISOString()}
*Logs:* ${url}
`;
}