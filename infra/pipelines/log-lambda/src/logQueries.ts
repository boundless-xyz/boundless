export const SERVICE_TO_QUERY_STRING_MAPPING = (service: string, logGroup: string, metricAlarmName: string) => {
  switch (service) {
    case 'bento-prover':
      // Note we have to escape two backslashes in the query string. In
      // CW it should look like: regexp_replace(log, '\\x1b\\[[0-9;]*[mK]', '') AS msg
      return `
SELECT
  \`@timestamp\`,
  regexp_replace(log, '\\\\x1b\\\\[[0-9;]*[mK]', '') AS msg
FROM \`${logGroup}\`
--WHERE
-- msg LIKE '%order_picker%' -- Filter to services
-- AND msg LIKE '%ERROR%' -- Only see error logs
ORDER BY \`@timestamp\` ASC -- Note this uses the timestamps that Cloudwatch received the log at, not the original timestamp
`.trim();
    case 'bonsai-prover':
      const a = `
parse @message /Z\\s+(?<log_level>[A-Z]+)\\s+(?<service>.+?):\\s+(?<message>.+)/
| display @timestamp, log_level, service, message
# | filter service like 'proving' and log_level like 'ERROR'
| sort @timestamp asc
`.trim();
      return a;
    case 'monitor':
      return `
fields @timestamp, level, fields.metric_time as PublishToCloudwatchTime, fields.message as msg, fields.requests as requests
| filter ispresent(level) # Remove lambda sys logs
| filter @message like 'Found'
#| filter @message like '0x8934790e351cbcadd11fc6f9729257cd64f860bf' # Filter to a specific client/prover
#| filter level like 'ERROR'
| sort @timestamp asc
`.trim();
    case 'indexer':
      return `
fields @timestamp, level, fields.message as msg
# | filter level like 'ERROR'
| sort @timestamp asc
`.trim();
    case 'og-offchain':
      return `
fields @timestamp, level, fields.message as msg
# | filter level like 'ERROR'
| sort @timestamp asc
`.trim();
    case 'og-onchain':
      return `
fields @timestamp, level, fields.message as msg
# | filter level like 'ERROR'
| sort @timestamp asc
`.trim();
    case 'order-stream':
      return `
fields @timestamp, @message
# | filter @message like 'ERROR'
| sort @timestamp asc
`.trim();
    default:
      return `
fields @timestamp, @message
# | filter @message like 'ERROR'
| sort @timestamp asc
`.trim();
  }
} 