import * as pulumi from "@pulumi/pulumi";
import { LaunchBasePipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LSlasherPipelineArgs extends BasePipelineArgs { }

const config: LaunchPipelineConfig = {
  appName: "slasher",
  buildTimeout: 60,
  computeType: "BUILD_GENERAL1_MEDIUM"
};

export class LSlasherPipeline extends LaunchBasePipeline {
  constructor(name: string, args: LSlasherPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-slasherPipeline`, name, config, args, opts);
  }
}