import * as pulumi from "@pulumi/pulumi";
import { LaunchBasePipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LProverPipelineArgs extends BasePipelineArgs { }

// SSM Document Name for updating the EC2 Bento Prover
const bentoBrokerInstanceIdStackOutputKey = "bentoBrokerInstanceId";
const updateBentoBrokerPulumiOutputKey = "bentoBrokerUpdateCommandId";

const config: LaunchPipelineConfig = {
  appName: "prover",
  buildTimeout: 180,
  computeType: "BUILD_GENERAL1_LARGE",
  postBuildCommands: [
    'echo "Updating EC2 Bento Prover"',
    `export SSM_DOCUMENT_NAME=$(pulumi stack output ${updateBentoBrokerPulumiOutputKey})`,
    `export INSTANCE_ID=$(pulumi stack output ${bentoBrokerInstanceIdStackOutputKey})`,
    'echo "INSTANCE_ID $INSTANCE_ID"',
    'echo "SSM_DOCUMENT_NAME $SSM_DOCUMENT_NAME"',
    'aws ssm send-command --document-name $SSM_DOCUMENT_NAME --targets "Key=InstanceIds,Values=$INSTANCE_ID" --cloud-watch-output-config CloudWatchOutputEnabled=true'
  ]
};

export class LProverPipeline extends LaunchBasePipeline {
  constructor(name: string, args: LProverPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-proverPipeline`, name, config, args, opts);
  }
}