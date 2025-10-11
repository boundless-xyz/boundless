import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./BasePipelineArgs";

export interface LaunchPipelineConfig {
  appName: string;
  buildTimeout?: number;
  computeType?: string;
  additionalBuildSpecCommands?: string[];
  postBuildCommands?: string[];
  branchName?: string;
}

export const LAUNCH_PIPELINE_DEFAULTS = {
  buildTimeout: 60,
  computeType: "BUILD_GENERAL1_MEDIUM",
  branchName: "main",
} as const;

export interface SecretsResult {
  githubTokenSecret: aws.secretsmanager.Secret;
  dockerTokenSecret: aws.secretsmanager.Secret;
}

export abstract class LaunchBasePipeline<TConfig extends LaunchPipelineConfig> extends pulumi.ComponentResource {
  protected readonly config: TConfig;

  constructor(
    type: string,
    name: string,
    config: TConfig,
    args: BasePipelineArgs,
    opts?: pulumi.ComponentResourceOptions
  ) {
    super(type, name, args, opts);
    this.config = {
      ...LAUNCH_PIPELINE_DEFAULTS,
      ...config
    } as TConfig;
    this.createPipeline(args);
  }

  protected abstract createPipeline(args: BasePipelineArgs): void;
  protected abstract getBuildSpec(): string;
  protected abstract codeBuildProjectArgs(
    appName: string,
    stackName: string,
    role: aws.iam.Role,
    serviceAccountRoleArn: string,
    dockerUsername: string,
    dockerTokenSecret: aws.secretsmanager.Secret,
    githubTokenSecret: aws.secretsmanager.Secret
  ): aws.codebuild.ProjectArgs;

  protected createSecretsAndPolicy(
    role: aws.iam.Role,
    githubToken: pulumi.Output<string>,
    dockerToken: pulumi.Output<string>
  ): SecretsResult {
    // These tokens are needed to avoid being rate limited by Github/Docker during the build process.
    const githubTokenSecret = new aws.secretsmanager.Secret(`l-${this.config.appName}-ghToken`);
    const dockerTokenSecret = new aws.secretsmanager.Secret(`l-${this.config.appName}-dockerToken`);

    new aws.secretsmanager.SecretVersion(`l-${this.config.appName}-ghTokenVersion`, {
      secretId: githubTokenSecret.id,
      secretString: githubToken,
    });

    new aws.secretsmanager.SecretVersion(`l-${this.config.appName}-dockerTokenVersion`, {
      secretId: dockerTokenSecret.id,
      secretString: dockerToken,
    });

    new aws.iam.RolePolicy(`l-${this.config.appName}-build-secrets`, {
      role: role.id,
      policy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
            Resource: [githubTokenSecret.arn, dockerTokenSecret.arn],
          },
        ],
      },
    });

    return { githubTokenSecret, dockerTokenSecret };
  }
}
