import { stringify } from "jsurl";

interface CloudWatchLogsInsightsParams {
  region: string;
  logGroupName: string;
  startTime: Date;
  endTime: Date;
  queryString: string;
  accountId: string;
}

export const encodeCloudWatchLogsInsightsUrl = async (params: CloudWatchLogsInsightsParams): Promise<string> => {
  const { region, logGroupName, startTime, endTime, queryString, accountId } = params;

  const lang = queryString.includes('SELECT') ? 'SQL' : 'CWLI';

  const queryMap = {
    editorString: queryString,
    end: endTime.toISOString(),
    source: `arn:aws:logs:${region}:${accountId}:log-group:${logGroupName}`,
    start: startTime.toISOString(),
    timeType: 'ABSOLUTE',
    tz: 'LOCAL',
    queryId: crypto.randomUUID(),
    lang: lang
  };

  const encodedQueryDetail2 = stringify(queryMap);
  const urlPrefix = encodeURIComponent(`https://console.aws.amazon.com/cloudwatch/home?region=${region}#logsV2:logs-insights`);
  const url = `${urlPrefix}$3FqueryDetail$3D${encodedQueryDetail2}`;
  console.log("Cloudwatch Logs Insights URL:", url);
  return url;
};

export const encodeAwsConsoleUrl = (baseUrl: string, unEncodedParams: Record<string, string>, preEncodedParams: Record<string, string>): string => {
  const params1 = Object.entries(unEncodedParams)
    .map(([key, value]) => `${key}=${encodeURIComponent(value)}`);

  const params2 = Object.entries(preEncodedParams)
    .map(([key, value]) => `${key}=${value}`);

  const encodedParams = params1.concat(params2).join('&');

  return `${baseUrl}?${encodedParams}`;
}; 