import * as pulumi from "@pulumi/pulumi";
import { LaunchDefaultPipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LOrderGeneratorPipelineArgs extends BasePipelineArgs { }

const config: LaunchPipelineConfig = {
  appName: "order-generator",
  buildTimeout: 60,
  computeType: "BUILD_GENERAL1_MEDIUM"
};

export class LOrderGeneratorPipeline extends LaunchDefaultPipeline {
  constructor(name: string, args: LOrderGeneratorPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-order-generatorPipeline`, name, config, args, opts);
  }
}