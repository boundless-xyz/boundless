// Run through test-producer-stop.py so each case uses its project's locked runtime.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const ts = require('typescript');
require.extensions['.ts'] = (module, filename) => module._compile(ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
  compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020, esModuleInterop: true},
}).outputText, filename);
const pulumi = require('@pulumi/pulumi');
const [kind, mode] = process.argv.slice(2);
const stopped = mode === 'stopped';
const root = path.resolve(__dirname, '..');
const resources = [];
pulumi.runtime.setMocks({
  newResource: args => {
    const arn = `arn:aws:mock:us-west-2:123456789012:${args.name}`;
    resources.push({type: args.type, name: args.name, inputs: args.inputs});
    return {id: `${args.name}-id`, state: {...args.inputs, arn, name: args.inputs.name || args.name,
      registryId: '123456789012', repositoryUrl: 'example.test/repo',
      repository: {repositoryUrl: 'example.test/repo', registryId: '123456789012'},
      taskDefinition: {arn}, outputs: {VPC_ID: 'vpc-test', PRIVATE_SUBNET_IDS: ['subnet-a']}}};
  },
  call: args => ({...args.inputs, id: 'ami-test', name: 'us-west-2', accountId: '123456789012',
    authorizationToken: Buffer.from('AWS:test').toString('base64'), proxyEndpoint: 'https://example.test',
    password: 'test', userName: 'AWS', json: '{}', repositoryUrl: 'example.test/repo'}),
}, 'test-project', 'staging-84532', false);
const strings = {CHAIN_ID: '84532', BASE_STACK: 'test/base', LOG_LEVEL: 'info', DOCKER_DIR: '.', DOCKER_TAG: 'test', KAILUA_IMAGE: 'test/image'};
pulumi.Config.prototype.require = function(key) {return strings[key] || (key === 'region' ? 'us-west-2' : '1');};
pulumi.Config.prototype.requireSecret = function() {return pulumi.secret('test-value');};
pulumi.Config.prototype.get = function() {return undefined;};
pulumi.Config.prototype.getSecret = function() {return undefined;};
pulumi.Config.prototype.getNumber = function(key) {
  if (key === 'DESIRED_COUNT') return stopped ? 0 : 1;
  if (mode === 'idle' && key === 'SCHEDULE_COUNT') return 0;
  return undefined;
};
pulumi.Config.prototype.getBoolean = function(key) {return key === 'STOPPED' ? stopped : undefined;};
const out = pulumi.output('test');
const args = {chainId: '84532', stackName: 'staging-84532', privateKey: out, pinataJWT: out, ethRpcUrl: out,
  indexerUrl: out, orderStreamUrl: out, imageRef: out, logLevel: 'info', setVerifierAddr: '0x1',
  boundlessMarketAddr: '0x2', ipfsGateway: 'https://test.invalid', lockCollateralRaw: '0',
  vpcId: out, privateSubnetIds: pulumi.output(['subnet-a']), desiredCount: 2};
async function main() {
  await pulumi.runtime.runInPulumiStack(async () => {
    if (kind === 'og-service' || kind === 'og-schedule') {
      const {OrderGenerator} = require('../infra/order-generator/components/order-generator.ts');
      if (kind === 'og-schedule') args.oneShotConfig = {count: '1', scheduleExpression: 'rate(5 minutes)'};
      new OrderGenerator('test', args);
    } else if (kind === 'load-test') {
      const {LoadTestTrigger} = require('../infra/order-generator/components/load-test.ts');
      new LoadTestTrigger(args);
    } else if (kind === 'indexer') {
      const {alarmConfig} = require('../infra/indexer/alarmConfig.ts');
      const {ChainId, Stage} = require('../infra/util/index.ts');
      const {buildCreateMetricFns} = require('../infra/indexer/components/alarms.ts');
      const producerNames = new Set(['og_offchain', 'og_onchain', 'signal_requestor', 'signal_sepolia_requestor', 'kailua_order_generator', 'kailua_og_offchain_2']);
      for (const chain of Object.values(ChainId)) for (const stage of Object.values(Stage)) {
        const current = alarmConfig[chain][stage];
        if (!current) continue;
        const {createMetricAlarm, createSuccessRateAlarm} = buildCreateMetricFns(`test-${chain}-${stage}`, 'test', []);
        for (const client of current.clients) {
          const expectedMuted = producerNames.has(client.name);
          for (const [key, alarms] of Object.entries(client)) {
            if (!Array.isArray(alarms)) continue;
            for (const cfg of alarms) {
              assert.equal(cfg.alarmConfig.actionsEnabled === false, expectedMuted);
              if (key === 'successRate') createSuccessRateAlarm(client, cfg.severity, cfg.description, cfg.metricConfig, cfg.alarmConfig, cfg.autoInvestigate);
              else createMetricAlarm({metricName: key, target: client, ...cfg});
            }
          }
        }
        for (const [key, alarms] of Object.entries(current.topLevel)) {
          const expectedMuted = ['submittedRequests', 'fulfilledRequests'].includes(key);
          for (const cfg of alarms) {
            assert.equal(cfg.alarmConfig.actionsEnabled === false, expectedMuted);
            createMetricAlarm({metricName: key, ...cfg});
          }
        }
      }
    } else {
      const dir = kind === 'kailua' ? 'kailua-batch' : kind === 'zeth' ? 'zeth-requestor' : kind;
      process.chdir(path.join(root, 'infra', dir));
      require(path.join(root, 'infra', dir, 'index.ts'))();
    }
  });
  await pulumi.runtime.disconnect();
  const select = type => resources.filter(r => r.type === type);
  const alarms = select('aws:cloudwatch/metricAlarm:MetricAlarm');
  if (kind === 'og-service') {
    assert.equal(select('awsx:ecs:FargateService').length, 1);
    assert.equal(select('awsx:ecs:FargateService')[0].inputs.desiredCount, stopped ? 0 : 2);
  }
  if (kind === 'og-schedule') {
    const rules = select('aws:cloudwatch/eventRule:EventRule'); assert.equal(rules.length, 1);
    assert.equal(rules[0].inputs.state, stopped ? 'DISABLED' : 'ENABLED');
  }
  if (kind === 'signal' || kind === 'zeth') {
    const services = select('awsx:ecs:FargateService'); assert.equal(services.length, kind === 'signal' ? 2 : 3);
    for (const service of services) assert.equal(service.inputs.desiredCount, stopped ? 0 : 1);
  }
  if (kind === 'load-test' || kind === 'kailua') {
    const lambdas = select('aws:lambda/function:Function'); assert.equal(lambdas.length, kind === 'kailua' ? 2 : 1);
    for (const lambda of lambdas) assert.equal(lambda.inputs.reservedConcurrentExecutions, stopped ? 0 : undefined);
  }
  if (kind === 'kailua') {
    const schedules = select('aws:scheduler/schedule:Schedule'); assert.equal(schedules.length, 2);
    for (const schedule of schedules) {
      const disabled = stopped || (mode === 'idle' && !schedule.name.includes('watchdog'));
      assert.equal(schedule.inputs.state, disabled ? 'DISABLED' : 'ENABLED');
    }
  }
  if (kind !== 'indexer' && kind !== 'load-test') {
    assert.ok(alarms.length, 'must exercise alarm muting');
    for (const alarm of alarms) {
      const idleLaunchAlarm = mode === 'idle' && alarm.name.includes('not-running');
      assert.equal(alarm.inputs.actionsEnabled, !stopped && !idleLaunchAlarm);
    }
  }
  if (kind === 'indexer') {
    assert.ok(alarms.some(a => a.inputs.actionsEnabled === true), 'customer and market failure alarms must remain enabled');
    assert.equal(alarms.filter(a => a.inputs.actionsEnabled === false).length, 19, 'must mute exactly the intended producer and volume alarms');
  }
  const stableResources = resources.map(r => {
    const inputs = {...r.inputs};
    if (r.type === 'awsx:ecs:FargateService') delete inputs.desiredCount;
    if (r.type === 'aws:lambda/function:Function') delete inputs.reservedConcurrentExecutions;
    if (['aws:cloudwatch/eventRule:EventRule', 'aws:scheduler/schedule:Schedule'].includes(r.type)) delete inputs.state;
    if (r.type === 'aws:cloudwatch/metricAlarm:MetricAlarm') delete inputs.actionsEnabled;
    return {...r, inputs};
  }).sort((a, b) => (a.type + a.name).localeCompare(b.type + b.name));
  console.log(JSON.stringify({kind, mode, resourceKeys: resources.map(r => r.type + '::' + r.name).sort(),
    nonControlHash: crypto.createHash('sha256').update(JSON.stringify(stableResources)).digest('hex'), status: 'passed'}));
}
main().catch(error => {console.error(error); process.exitCode = 1;});
