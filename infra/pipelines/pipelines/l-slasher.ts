import * as pulumi from "@pulumi/pulumi";
import { LaunchDefaultPipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LSlasherPipelineArgs extends BasePipelineArgs { }

const config: LaunchPipelineConfig = {
  appName: "slasher",
  buildTimeout: 60,
  computeType: "BUILD_GENERAL1_MEDIUM"
};

export class LSlasherPipeline extends LaunchDefaultPipeline {
  constructor(name: string, args: LSlasherPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-slasherPipeline`, name, config, args, opts);
  }
}