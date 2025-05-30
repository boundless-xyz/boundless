import { processAlarmEvent } from '../src';
import { buildCloudWatchLogsInsightsUrl } from '../src/urls';
import { CloudWatchClient } from '@aws-sdk/client-cloudwatch';

const ssoBaseUrl = 'https://d-9a671bbc7a.awsapps.com/start/#/console';

// SAMPLE URL https://console.aws.amazon.com/cloudwatch/home?region=us-west-2#logsV2:logs-insights$3FqueryDetail$3D~(end~'2025-05-30T05*3a00*3a00.000Z~start~'2025-05-30T04*3a00*3a00.000Z~timeType~'ABSOLUTE~tz~'LOCAL~editorString~'fields*20*40timestamp*2c*20*40message*2c*20*40logStream*2c*20*40log*0a*7c*20sort*20*40timestamp*20desc*0a*7c*20limit*2010000~queryId~'66cfc23a-37bd-44b6-b0fd-dd12d4a40cdc~source~(~'arn*3aaws*3alogs*3aus-west-2*3a632745187633*3alog-group*3aprod-11155111-bonsai-prover-11155111)~lang~'CWLI)
// SMAPLE https://d-9a671bbc7a.awsapps.com/start/#/console?account_id=632745187633&role_name=AWSPowerUserAccess&destination=https%3A%2F%2Fconsole.aws.amazon.com%2Fcloudwatch%2Fhome%3Fregion%3Dus-west-2%23logsV2%3Alogs-insights%243FqueryDetail%243D~(end~'2025-05-29T15*3a26*3a11.000Z~start~'2025-05-29T15*3a23*3a11.000Z~timeType~'ABSOLUTE~tz~'LOCAL~editorString~'SELECT*0a*20*20regexp_replace(log*2c*20*27%5Cx1b%5C*5b*5b0-9%3B*5d**5bmK*5d*27*2c*20*27*27)*20AS*20cleaned_log*2c*0a*20*20timestamp*0aFROM*20%60prod-11155111-bento-prover-11155111%60*0aWHERE*20log*20LIKE*20*27%25Order%25*27*0aLIMIT*2020~queryId~'30b6bcca-ebd1-4dda-9507-fa3245604190~source~(~'arn*3aaws*3alogs*3aus-west-2*3a632745187633*3alog-group*3aprod-11155111-bento-prover-11155111)~lang~'SQL)
/** SAMPLE ALARM EVENT
 {
    "alarmArn": "arn:aws:cloudwatch:us-west-2:632745187633:alarm:prod-11155111-bento-prover-11155111-order-monitor-unexpected-error-SEV2",
    "namespace": "Boundless/Services/prod-11155111-bento-prover-11155111",
    "alarmDescription": "SEV2 order-monitor-unexpected-error ",
    "timestamp": "Thu, 29 May 2025 15:25:11 UTC",
    "metricAlarmName": "prod-11155111-bento-prover-11155111-order-monitor-unexpected-error-SEV2",
    "metric": "prod-11155111-bento-prover-11155111-order-monitor-unexpected-error-SEV2",
    "alarmState": "ALARM"
}
*/

describe('CloudWatch Logs Insights URL Builder', () => {
  it('should generate the correct URL format', async () => {
    const params = {
      region: 'us-west-2',
      logGroupName: 'prod-11155111-bonsai-prover-11155111',
      startTime: new Date('2025-05-30T04:00:00.000Z'),
      endTime: new Date('2025-05-30T05:00:00.000Z'),
      queryString: 'fields @timestamp, @message, @logStream, @log\n| sort @timestamp desc\n| limit 10000',
      accountId: '632745187633'
    };

    const url = await buildCloudWatchLogsInsightsUrl(params);
    const expectedUrl = 'https://console.aws.amazon.com/cloudwatch/home?region=us-west-2#logsV2:logs-insights$3FqueryDetail$3D~(end~\'2025-05-30T05*3a00*3a00.000Z~start~\'2025-05-30T04*3a00*3a00.000Z~timeType~\'ABSOLUTE~tz~\'LOCAL~editorString~\'fields*20*40timestamp*2c*20*40message*2c*20*40logStream*2c*20*40log*0a*7c*20sort*20*40timestamp*20desc*0a*7c*20limit*2010000~queryId~\'66cfc23a-37bd-44b6-b0fd-dd12d4a40cdc~source~(~\'arn*3aaws*3alogs*3aus-west-2*3a632745187633*3alog-group*3aprod-11155111-bonsai-prover-11155111)~lang~\'CWLI)';

    // Test exact match
    expect(url).toBe(expectedUrl);

    // Test individual components
    expect(url).toContain('region=us-west-2');
    expect(url).toContain('logsV2:logs-insights');
    expect(url).toContain('end~\'2025-05-30T05*3a00*3a00.000Z');
    expect(url).toContain('start~\'2025-05-30T04*3a00*3a00.000Z');
    expect(url).toContain('timeType~\'ABSOLUTE');
    expect(url).toContain('tz~\'LOCAL');
    expect(url).toContain('fields*20*40timestamp*2c*20*40message*2c*20*40logStream*2c*20*40log*0a*7c*20sort*20*40timestamp*20desc*0a*7c*20limit*2010000');
    expect(url).toContain('source~(~\'arn*3aaws*3alogs*3aus-west-2*3a632745187633*3alog-group*3aprod-11155111-bonsai-prover-11155111)');
    expect(url).toContain('lang~\'CWLI');
  });
});

describe('processAlarmEvent', () => {
  it('should correctly parse a metric alarm name with hyphenated service', async () => {
    const event = {
      "alarmArn": "arn:aws:cloudwatch:us-west-2:632745187633:alarm:prod-11155111-bento-prover-11155111-order-monitor-unexpected-error-SEV2",
      "namespace": "Boundless/Services/prod-11155111-bento-prover-11155111",
      "alarmDescription": "SEV2 order-monitor-unexpected-error ",
      "timestamp": "Thu, 29 May 2025 15:25:11 UTC",
      "metricAlarmName": "prod-11155111-bento-prover-11155111-order-monitor-unexpected-error-SEV2",
      "metric": "prod-11155111-bento-prover-11155111-order-monitor-unexpected-error-SEV2",
      "alarmState": "ALARM"
    };

    const result = await processAlarmEvent(ssoBaseUrl, new CloudWatchClient({ region: 'us-west-2' }), event);
    console.log(result);
    expect(result).toContain('*Stage:* prod');
    expect(result).toContain('*Chain ID:* Ethereum Sepolia (11155111)');
    expect(result).toContain('*Service:* bento-prover');
    expect(result).toContain('*Logs:* https://console.aws.amazon.com/cloudwatch');
  });

  it('should correctly parse a metric alarm name with single word service', async () => {
    const event = {
      alarmArn: 'test-arn',
      namespace: 'test-namespace',
      alarmDescription: 'test-description',
      timestamp: 'test-timestamp',
      metricAlarmName: 'prod-8453-monitor-8453-requests_number_from_0x2546c553d857d20658ece248f7c7d0861a240681-SEV',
      metric: 'test-metric',
      alarmState: 'ALARM'
    };

    const result = await processAlarmEvent(ssoBaseUrl, new CloudWatchClient({ region: 'us-west-2' }), event);
    expect(result).toContain('*Stage:* prod');
    expect(result).toContain('*Chain ID:* 8453');
    expect(result).toContain('*Service:* monitor');
    expect(result).toContain('*Logs:* https://console.aws.amazon.com/cloudwatch');
  });
}); 