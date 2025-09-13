import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN } from "../accountConstants";
import { BasePipelineArgs } from "./base";

export interface LaunchPipelineConfig {
  appName: string;
  buildTimeout?: number; // Default: 60
  computeType?: string; // Default: "BUILD_GENERAL1_MEDIUM"
  additionalBuildSpecCommands?: string[]; // For Rust/cargo setup in pre_build
  postBuildCommands?: string[]; // For EC2 management in post_build
}

export abstract class LaunchBasePipeline extends pulumi.ComponentResource {
  protected readonly BRANCH_NAME = "main";
  protected readonly config: LaunchPipelineConfig;

  constructor(
    type: string,
    name: string,
    config: LaunchPipelineConfig,
    args: BasePipelineArgs,
    opts?: pulumi.ComponentResourceOptions
  ) {
    super(type, name, args, opts);
    this.config = {
      buildTimeout: 60,
      computeType: "BUILD_GENERAL1_MEDIUM",
      ...config
    };
    this.createPipeline(args);
  }

  private createPipeline(args: BasePipelineArgs) {
    const { connection, artifactBucket, role, githubToken, dockerUsername, dockerToken, slackAlertsTopicArn } = args;

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

    // Create CodeBuild projects for each stack
    const stagingDeploymentBaseSepolia = new aws.codebuild.Project(
      `l-${this.config.appName}-staging-84532-build`,
      this.codeBuildProjectArgs(this.config.appName, "l-staging-84532", role, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
      { dependsOn: [role] }
    );

    const prodDeploymentBaseSepolia = new aws.codebuild.Project(
      `l-${this.config.appName}-prod-84532-build`,
      this.codeBuildProjectArgs(this.config.appName, "l-prod-84532", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
      { dependsOn: [role] }
    );

    const prodDeploymentBaseMainnet = new aws.codebuild.Project(
      `l-${this.config.appName}-prod-8453-build`,
      this.codeBuildProjectArgs(this.config.appName, "l-prod-8453", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
      { dependsOn: [role] }
    );

    const prodDeploymentEthSepolia = new aws.codebuild.Project(
      `l-${this.config.appName}-prod-11155111-build`,
      this.codeBuildProjectArgs(this.config.appName, "l-prod-11155111", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
      { dependsOn: [role] }
    );

    // Create the pipeline
    const pipeline = new aws.codepipeline.Pipeline(`l-${this.config.appName}-pipeline`, {
      pipelineType: "V2",
      artifactStores: [{
        type: "S3",
        location: artifactBucket.bucket
      }],
      stages: [
        {
          name: "Github",
          actions: [{
            name: "Github",
            category: "Source",
            owner: "AWS",
            provider: "CodeStarSourceConnection",
            version: "1",
            outputArtifacts: ["source_output"],
            configuration: {
              ConnectionArn: connection.arn,
              FullRepositoryId: "boundless-xyz/boundless",
              BranchName: this.BRANCH_NAME,
              OutputArtifactFormat: "CODEBUILD_CLONE_REF"
            },
          }],
        },
        {
          name: "DeployStaging",
          actions: [
            {
              name: "DeployStagingBaseSepolia",
              category: "Build",
              owner: "AWS",
              provider: "CodeBuild",
              version: "1",
              runOrder: 1,
              configuration: {
                ProjectName: stagingDeploymentBaseSepolia.name
              },
              outputArtifacts: ["staging_output_base_sepolia"],
              inputArtifacts: ["source_output"],
            }
          ]
        },
        {
          name: "DeployProduction",
          actions: [
            {
              name: "ApproveDeployToProduction",
              category: "Approval",
              owner: "AWS",
              provider: "Manual",
              version: "1",
              runOrder: 1,
              configuration: {}
            },
            {
              name: "DeployProductionBaseSepolia",
              category: "Build",
              owner: "AWS",
              provider: "CodeBuild",
              version: "1",
              runOrder: 2,
              configuration: {
                ProjectName: prodDeploymentBaseSepolia.name
              },
              outputArtifacts: ["production_output_base_sepolia"],
              inputArtifacts: ["source_output"],
            },
            {
              name: "DeployProductionEthSepolia",
              category: "Build",
              owner: "AWS",
              provider: "CodeBuild",
              version: "1",
              runOrder: 2,
              configuration: {
                ProjectName: prodDeploymentEthSepolia.name
              },
              outputArtifacts: ["production_output_eth_sepolia"],
              inputArtifacts: ["source_output"],
            },
            {
              name: "DeployProductionBaseMainnet",
              category: "Build",
              owner: "AWS",
              provider: "CodeBuild",
              version: "1",
              runOrder: 2,
              configuration: {
                ProjectName: prodDeploymentBaseMainnet.name
              },
              outputArtifacts: ["production_output_base_mainnet"],
              inputArtifacts: ["source_output"],
            }
          ]
        }
      ],
      triggers: [
        {
          providerType: "CodeStarSourceConnection",
          gitConfiguration: {
            sourceActionName: "Github",
            pushes: [
              {
                branches: {
                  includes: [this.BRANCH_NAME],
                },
              },
            ],
          },
        },
      ],
      name: `l-${this.config.appName}-pipeline`,
      roleArn: role.arn,
    });

    // Create notification rule
    new aws.codestarnotifications.NotificationRule(`l-${this.config.appName}-pipeline-notifications`, {
      name: `l-${this.config.appName}-pipeline-notifications`,
      eventTypeIds: [
        "codepipeline-pipeline-manual-approval-succeeded",
        "codepipeline-pipeline-action-execution-failed",
      ],
      resource: pipeline.arn,
      detailType: "FULL",
      targets: [
        {
          address: slackAlertsTopicArn.apply(arn => arn),
        },
      ],
    });
  }

  protected getBuildSpec(): string {
    const additionalCommands = this.config.additionalBuildSpecCommands || [];
    const postBuildCommands = this.config.postBuildCommands || [];

    const additionalCommandsStr = additionalCommands.length > 0
      ? additionalCommands.map(cmd => `          - ${cmd}`).join('\n') + '\n'
      : '';

    const postBuildSection = postBuildCommands.length > 0
      ? `      post_build:
        commands:
${postBuildCommands.map(cmd => `          - ${cmd}`).join('\n')}`
      : '';

    return `
    version: 0.2

    env:
      git-credential-helper: yes
    
    phases:
      pre_build:
        commands:
          - echo Assuming role $DEPLOYMENT_ROLE_ARN
          - ASSUMED_ROLE=$(aws sts assume-role --role-arn $DEPLOYMENT_ROLE_ARN --role-session-name Deployment --output text | tail -1)
          - export AWS_ACCESS_KEY_ID=$(echo $ASSUMED_ROLE | awk '{print $2}')
          - export AWS_SECRET_ACCESS_KEY=$(echo $ASSUMED_ROLE | awk '{print $4}')
          - export AWS_SESSION_TOKEN=$(echo $ASSUMED_ROLE | awk '{print $5}')
          - curl -fsSL https://get.pulumi.com/ | sh -s -- --version 3.193.0
          - export PATH=$PATH:$HOME/.pulumi/bin
          - pulumi login --non-interactive "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
          - git submodule update --init --recursive
          - echo $DOCKER_PAT > docker_token.txt
          - cat docker_token.txt | docker login -u $DOCKER_USERNAME --password-stdin
${additionalCommandsStr}          - ls -lt
      build:
        commands:
          - cd infra/$APP_NAME
          - pulumi install
          - echo "DEPLOYING stack $STACK_NAME"
          - pulumi stack select $STACK_NAME
          - pulumi cancel --yes
          - pulumi up --yes${postBuildSection ? '\n' + postBuildSection : ''}
    `;
  }

  private codeBuildProjectArgs(
    appName: string,
    stackName: string,
    role: aws.iam.Role,
    serviceAccountRoleArn: string,
    dockerUsername: string,
    dockerTokenSecret: aws.secretsmanager.Secret,
    githubTokenSecret: aws.secretsmanager.Secret
  ): aws.codebuild.ProjectArgs {
    return {
      buildTimeout: this.config.buildTimeout!,
      description: `Launch deployment for ${this.config.appName}`,
      serviceRole: role.arn,
      environment: {
        computeType: this.config.computeType!,
        image: "aws/codebuild/standard:7.0",
        type: "LINUX_CONTAINER",
        privilegedMode: true,
        environmentVariables: [
          {
            name: "DEPLOYMENT_ROLE_ARN",
            type: "PLAINTEXT",
            value: serviceAccountRoleArn
          },
          {
            name: "STACK_NAME",
            type: "PLAINTEXT",
            value: stackName
          },
          {
            name: "APP_NAME",
            type: "PLAINTEXT",
            value: appName
          },
          {
            name: "GITHUB_TOKEN",
            type: "SECRETS_MANAGER",
            value: githubTokenSecret.name
          },
          {
            name: "DOCKER_USERNAME",
            type: "PLAINTEXT",
            value: dockerUsername
          },
          {
            name: "DOCKER_PAT",
            type: "SECRETS_MANAGER",
            value: dockerTokenSecret.name
          }
        ]
      },
      artifacts: { type: "CODEPIPELINE" },
      source: {
        type: "CODEPIPELINE",
        buildspec: this.getBuildSpec()
      }
    }
  }
}