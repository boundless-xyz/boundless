import * as pulumi from "@pulumi/pulumi";
import { LaunchDefaultPipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LProverPipelineArgs extends BasePipelineArgs { }

// SSM Document Name for updating the EC2 Bento Prover
const bentoBroker1InstanceIdStackOutputKey = "bentoBroker1InstanceId";
const updateBentoBroker1PulumiOutputKey = "bentoBroker1UpdateCommandId";
const bentoBroker2InstanceIdStackOutputKey = "bentoBroker2InstanceId";
const updateBentoBroker2PulumiOutputKey = "bentoBroker2UpdateCommandId";

const config: LaunchPipelineConfig = {
  appName: "prover",
  buildTimeout: 180,
  computeType: "BUILD_GENERAL1_LARGE",
  postBuildCommands: [
    'echo "Updating EC2 Bento Prover 1"',
    `export SSM_DOCUMENT_NAME=$(pulumi stack output ${updateBentoBroker1PulumiOutputKey})`,
    `export INSTANCE_ID=$(pulumi stack output ${bentoBroker1InstanceIdStackOutputKey})`,
    'echo "INSTANCE_ID $INSTANCE_ID"',
    'echo "SSM_DOCUMENT_NAME $SSM_DOCUMENT_NAME"',
    'aws ssm send-command --document-name $SSM_DOCUMENT_NAME --targets "Key=InstanceIds,Values=$INSTANCE_ID" --cloud-watch-output-config CloudWatchOutputEnabled=true',
    'echo "Updating EC2 Bento Prover 2"',
    `export SSM_DOCUMENT_NAME=$(pulumi stack output ${updateBentoBroker2PulumiOutputKey})`,
    `export INSTANCE_ID=$(pulumi stack output ${bentoBroker2InstanceIdStackOutputKey})`,
    'echo "INSTANCE_ID $INSTANCE_ID"',
    'echo "SSM_DOCUMENT_NAME $SSM_DOCUMENT_NAME"',
    'aws ssm send-command --document-name $SSM_DOCUMENT_NAME --targets "Key=InstanceIds,Values=$INSTANCE_ID" --cloud-watch-output-config CloudWatchOutputEnabled=true'
  ]
};

export class LProverPipeline extends LaunchDefaultPipeline {
  constructor(name: string, args: LProverPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-proverPipeline`, name, config, args, opts);
  }
}