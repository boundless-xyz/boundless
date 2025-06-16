import { processAlarmEvent } from '../src';
import { encodeCloudWatchLogsInsightsUrl } from '../src/urls';
import { CloudWatchClient } from '@aws-sdk/client-cloudwatch';
import 'jest';

const ssoBaseUrl = 'https://test.awsapps.com/start/#/console';
const runbookUrl = 'https://TEST_RUNBOOK_URL.awsapps.com/start/#/console';

// SAMPLE URL https://console.aws.amazon.com/cloudwatch/home?region=us-west-2#logsV2:logs-insights$3FqueryDetail$3D~(end~'2025-05-30T05*3a00*3a00.000Z~start~'2025-05-30T04*3a00*3a00.000Z~timeType~'ABSOLUTE~tz~'LOCAL~editorString~'fields*20*40timestamp*2c*20*40message*2c*20*40logStream*2c*20*40log*0a*7c*20sort*20*40timestamp*20desc*0a*7c*20limit*2010000~queryId~'66cfc23a-37bd-44b6-b0fd-dd12d4a40cdc~source~(~'arn*3aaws*3alogs*3aus-west-2*3a632745187633*3alog-group*3aprod-11155111-bonsai-prover-11155111)~lang~'CWLI)
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

    const url = await encodeCloudWatchLogsInsightsUrl(params);

    // Test individual components
    expect(url).toContain('region%3Dus-west-2');
    expect(url).toContain('logsV2%3Alogs-insights');
    expect(url).toContain('end~\'2025-05-30T05*3a00*3a00.000Z');
    expect(url).toContain('start~\'2025-05-30T04*3a00*3a00.000Z');
    expect(url).toContain('timeType~\'ABSOLUTE');
    expect(url).toContain('tz~\'LOCAL');
    expect(url).toContain('fields*20*40timestamp*2c*20*40message*2c*20*40logStream*2c*20*40log*0a*7c*20sort*20*40timestamp*20desc*0a*7c*20limit*2010000');
    expect(url).toContain('632745187633*3alog-group*3aprod-111');
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

    const result = await processAlarmEvent(ssoBaseUrl, runbookUrl, new CloudWatchClient({ region: 'us-west-2' }), event);
    console.log(result);
    expect(result).toContain('prod');
    expect(result).toContain('11155111 (Ethereum Sepolia)');
    expect(result).toContain('bento-prover');
    expect(result).toContain('View Logs');
  });

  it('should correctly parse a metric alarm name with single word service', async () => {
    const event = {
      alarmArn: 'test-arn',
      namespace: 'test-namespace',
      alarmDescription: 'test-description',
      timestamp: "Thu, 29 May 2025 15:25:11 UTC",
      metricAlarmName: 'prod-8453-monitor-8453-requests_number_from_0x2546c553d857d20658ece248f7c7d0861a240681-SEV',
      metric: 'test-metric',
      alarmState: 'ALARM'
    };

    const result = await processAlarmEvent(ssoBaseUrl, runbookUrl, new CloudWatchClient({ region: 'us-west-2' }), event);
    expect(result).toContain('prod');
    expect(result).toContain('8453 (Base Mainnet)');
    expect(result).toContain('monitor');
    expect(result).toContain('test.awsapps.com/start/#/console');
  });

  it('should correctly parse a metric alarm name with format: stage-service-chainId-description-severity', async () => {
    const event = {
      alarmArn: 'arn:aws:cloudwatch:us-west-2:123456789012:alarm:test-alarm',
      namespace: 'AWS/Lambda',
      alarmDescription: 'Test alarm description',
      timestamp: '2024-03-20T12:00:00Z',
      metricAlarmName: 'staging-monitor-11155111-requests_number_from_0xe9669e8fe06aa27d3ed5d85a33453987c80bbdc3-SEV2',
      metric: 'Duration',
      alarmState: 'ALARM'
    };

    const result = await processAlarmEvent(ssoBaseUrl, runbookUrl, new CloudWatchClient({ region: 'us-west-2' }), event);
    expect(result).toContain('staging');
    expect(result).toContain('11155111 (Ethereum Sepolia)');
    expect(result).toContain('monitor');
    expect(result).toContain('Test alarm description');
  });

  it('should correctly parse a metric alarm name with additional suffixes', async () => {
    const event = {
      alarmArn: 'arn:aws:cloudwatch:us-west-2:123456789012:alarm:test-alarm',
      namespace: 'AWS/Lambda',
      alarmDescription: 'Test alarm description',
      timestamp: '2024-03-20T12:00:00Z',
      metricAlarmName: 'staging-11155111-og-onchain-11155111-log-fatal-SEV2',
      metric: 'Duration',
      alarmState: 'ALARM'
    };

    const result = await processAlarmEvent(ssoBaseUrl, runbookUrl, new CloudWatchClient({ region: 'us-west-2' }), event);
    expect(result).toContain('staging');
    expect(result).toContain('11155111 (Ethereum Sepolia)');
    expect(result).toContain('log-group*3astaging-11155111-og-onchain-11155111');
    expect(result).toContain('Test alarm description');
  });

}); 