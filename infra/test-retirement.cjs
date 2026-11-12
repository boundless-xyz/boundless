const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const service = process.argv[2];
const serviceRequire = require('node:module').createRequire(path.join(__dirname, service, 'package.json'));
const ts = serviceRequire('typescript');
const pulumi = serviceRequire('@pulumi/pulumi');

require.extensions['.ts'] = (module, filename) => module._compile(
  ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
    compilerOptions: {module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020},
  }).outputText,
  filename,
);

const indexerUsesOrderStream = process.argv[3] === 'indexer-enabled';
const indexerOutputMissing = process.argv[3] === 'indexer-missing';
assert(['distributor', 'slasher', 'order-stream', 'order-generator', 'requestor-lists', 'cw-monitoring'].includes(service));
const resources = [];
pulumi.runtime.setMocks({
  newResource(args) {
    resources.push(args);
    return {id: args.name, state: {
      ...args.inputs,
      ...(args.type === 'pulumi:pulumi:StackReference' ? {
        outputs: indexerOutputMissing ? {} : {ORDER_STREAM_ENABLED: indexerUsesOrderStream},
        secretOutputNames: [],
      } : {}),
    }};
  },
  call() { throw new Error('Retirement must not look up cloud resources'); },
}, service, service === 'requestor-lists' ? 'l-prod' : service === 'cw-monitoring' ? 'production' : 'l-prod-8453', false);
pulumi.Config.prototype.getBoolean = function (key) {
  assert.equal(key, 'RETIRED');
  return true;
};
pulumi.Config.prototype.require = function () { throw new Error('Service config was read'); };
pulumi.Config.prototype.requireSecret = function () { throw new Error('Wallet secret was read'); };
require('node:child_process').execSync = () => { throw new Error('Image setup was invoked'); };

(async () => {
  await pulumi.runtime.runInPulumiStack(async () => {
    assert.deepEqual(await require(path.join(__dirname, service, 'index.ts'))(), {});
  });
  await pulumi.runtime.disconnect();
  assert(resources.every(resource => ['pulumi:pulumi:Stack', 'pulumi:pulumi:StackReference'].includes(resource.type)));
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
