import * as pulumi from "@pulumi/pulumi";
import { LaunchBasePipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LDistributorPipelineArgs extends BasePipelineArgs { }

const config: LaunchPipelineConfig = {
  appName: "distributor",
  buildTimeout: 75,
  computeType: "BUILD_GENERAL1_LARGE"
};

export class LDistributorPipeline extends LaunchBasePipeline {
  constructor(name: string, args: LDistributorPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-distributorPipeline`, name, config, args, opts);
  }
}