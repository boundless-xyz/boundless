const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const projectRequire = require('node:module').createRequire(path.join(__dirname, 'pipelines/package.json'));
const ts = projectRequire('typescript');
const pulumi = projectRequire('@pulumi/pulumi');
const aws = projectRequire('@pulumi/aws');

require.extensions['.ts'] = (module, filename) => module._compile(
  ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020},
  }).outputText, filename,
);
const resources = [];
pulumi.runtime.setMocks({
  newResource(args) {
    resources.push(args);
    return {id: args.name, state: {...args.inputs, arn: `arn:aws:test:::${args.name}`, name: args.name}};
  },
  call() { throw new Error('Pipeline test must not call AWS'); },
}, 'pipelines', 'prod', false);

(async () => {
  await pulumi.runtime.runInPulumiStack(async () => {
    const {LIndexerPipeline} = require('./pipelines/pipelines/l-indexer.ts');
    new LIndexerPipeline('indexer', {
      connection: new aws.codestarconnections.Connection('connection', {providerType: 'GitHub'}),
      artifactBucket: new aws.s3.Bucket('artifacts'),
      role: new aws.iam.Role('role', {assumeRolePolicy: '{}'}),
      githubToken: pulumi.secret('test'), dockerToken: pulumi.secret('test'),
      dockerUsername: 'test', slackAlertsTopicArn: pulumi.output('arn:aws:sns:::test'),
    });
  });
  await pulumi.runtime.disconnect();
  const projects = resources.filter(r => r.type === 'aws:codebuild/project:Project');
  const stacks = projects.map(r => r.inputs.environment.environmentVariables.find(v => v.name === 'STACK_NAME').value).sort();
  assert.deepEqual(stacks, ['l-prod-1', 'l-prod-167000', 'l-prod-8453', 'l-staging-11155111', 'l-staging-167000', 'l-staging-84532'].sort());
  const pipeline = resources.find(r => r.type === 'aws:codepipeline/pipeline:Pipeline');
  const production = pipeline.inputs.stages.find(s => s.name === 'DeployProduction');
  assert.deepEqual(production.actions.map(a => a.name).sort(), ['ApproveDeployToProduction', 'DeployProductionBaseMainnet', 'DeployProductionEthMainnet', 'DeployProductionTaiko'].sort());
  assert(production.actions.filter(a => a.category === 'Build').every(a => a.runOrder === 2));
  assert.equal(production.actions.find(a => a.category === 'Approval').runOrder, 1);
})().catch(error => { console.error(error); process.exitCode = 1; });
