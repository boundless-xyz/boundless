import * as pulumi from "@pulumi/pulumi";
import { LaunchBasePipeline, LaunchPipelineConfig } from "./l-base";
import { BasePipelineArgs } from "./base";

interface LIndexerPipelineArgs extends BasePipelineArgs { }

const config: LaunchPipelineConfig = {
  appName: "indexer",
  buildTimeout: 60,
  computeType: "BUILD_GENERAL1_LARGE",
  additionalBuildSpecCommands: [
    'curl https://sh.rustup.rs -sSf | sh -s -- -y',
    '. "$HOME/.cargo/env"',
    'curl -fsSL https://cargo-lambda.info/install.sh | sh -s -- -y',
    '. "$HOME/.cargo/env"',
    'npm install -g @ziglang/cli'
  ]
};

export class LIndexerPipeline extends LaunchBasePipeline {
  constructor(name: string, args: LIndexerPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super(`boundless:pipelines:l-indexerPipeline`, name, config, args, opts);
  }
}