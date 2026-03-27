import * as pulumi from "@pulumi/pulumi";
import { LaunchDefaultPipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LOrderStreamPipelineArgs extends BasePipelineArgs { }

const config: LaunchPipelineConfig = {
  appName: "order-stream",
  buildTimeout: 60,
  computeType: "BUILD_GENERAL1_MEDIUM",
  branchName: "willpote/infra-taiko-v2",
  includeTaiko: true,
  postBuildCommands: [
    'pulumi stack select $STACK_NAME',
    './post-deploy.sh $STACK_NAME',
  ],
};

export class LOrderStreamPipeline extends LaunchDefaultPipeline {
  constructor(name: string, args: LOrderStreamPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-order-streamPipeline`, name, config, args, opts);
  }
}