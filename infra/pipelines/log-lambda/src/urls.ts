interface CloudWatchLogsInsightsParams {
  region: string;
  logGroupName: string;
  startTime: Date;
  endTime: Date;
  queryString: string;
  accountId: string;
}

export const buildCloudWatchLogsInsightsUrl = async (params: CloudWatchLogsInsightsParams): Promise<string> => {
  const { region, logGroupName, startTime, endTime, queryString, accountId } = params;

  // Format dates to ISO string and replace : with *3a
  const formatDate = (date: Date) => date.toISOString().replace(/:/g, '*3a');

  // Encode the query string to match CloudWatch's exact format
  const replacements: Record<string, string> = {
    '\n': '*0a',
    ' ': '*20',
    '(': '*28',
    ')': '*29',
    ',': '*2c',
    "'": '*27',
    '`': '*60',
    '%': '*25',
    '\\': '*5c',
    '[': '*5b',
    ']': '*5d',
    ';': '*3b',
    '*': '*2a'
  };

  // Process the string character by character
  const encodedQuery = queryString.split('').map(char => {
    if (char === '\\') {
      return '*5c*5c';
    }
    return replacements[char] || char;
  }).join('');

  const lang = queryString.includes('SELECT') ? 'SQL' : 'CWLI';
  const encodedQueryDetail = [
    `end~'${formatDate(endTime)}`,
    `start~'${formatDate(startTime)}`,
    `timeType~'ABSOLUTE`,
    `tz~'LOCAL`,
    `editorString~'${encodedQuery}`,
    // `queryId~'${crypto.randomUUID()}`,
    `source~(~'arn*3aaws*3alogs*3a${region}*3a${accountId}*3alog-group*3a${logGroupName})`,
    `lang~'${lang}`
  ].join('~');

  const urlPrefix = encodeURIComponent(`https://console.aws.amazon.com/cloudwatch/home?region=${region}#logsV2:logs-insights`);
  const url = `${urlPrefix}$3FqueryDetail$3D~(${encodedQueryDetail})`;
  console.log(url);
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