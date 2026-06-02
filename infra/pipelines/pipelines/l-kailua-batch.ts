import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BOUNDLESS_OPS_ACCOUNT_ID, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN } from "../accountConstants";
import { ASSUME_ROLE_CHAINED_MAX_SESSION_SECONDS } from "../../util";
import {
    BasePipelineArgs,
    LaunchBasePipeline,
    LaunchPipelineConfig,
    createLaunchPipelineNotificationRule,
    createProdStageFailureAlert,
} from "./l-base";

interface LKailuaBatchPipelineArgs extends BasePipelineArgs {}

const config: LaunchPipelineConfig = {
    appName: "kailua-batch",
    buildTimeout: 75,
    computeType: "BUILD_GENERAL1_MEDIUM",
    branchName: "main",
};

/** Staging/prod env stacks (Pulumi.staging.yaml, Pulumi.prod.yaml) — not per-chain. */
export class LKailuaBatchPipeline extends LaunchBasePipeline<LaunchPipelineConfig> {
    constructor(name: string, args: LKailuaBatchPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super(`boundless:pipelines:l-kailua-batchPipeline`, name, config, args, opts);
    }

    protected createPipeline(args: BasePipelineArgs) {
        const { connection, artifactBucket, role, githubToken, dockerUsername, dockerToken, slackAlertsTopicArn } = args;

        const { githubTokenSecret, dockerTokenSecret } = this.createSecretsAndPolicy(role, githubToken, dockerToken);

        const stagingDeployment = new aws.codebuild.Project(
            `l-${this.config.appName}-staging-build`,
            this.codeBuildProjectArgs(
                this.config.appName,
                "staging",
                role,
                BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
                dockerUsername,
                dockerTokenSecret,
                githubTokenSecret,
            ),
            { dependsOn: [role] },
        );

        const prodDeployment = new aws.codebuild.Project(
            `l-${this.config.appName}-prod-build`,
            this.codeBuildProjectArgs(
                this.config.appName,
                "prod",
                role,
                BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
                dockerUsername,
                dockerTokenSecret,
                githubTokenSecret,
            ),
            { dependsOn: [role] },
        );

        const pipeline = new aws.codepipeline.Pipeline(`l-${this.config.appName}-pipeline`, {
            pipelineType: "V2",
            artifactStores: [{
                type: "S3",
                location: artifactBucket.bucket,
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
                            BranchName: this.config.branchName!,
                            OutputArtifactFormat: "CODEBUILD_CLONE_REF",
                        },
                    }],
                },
                {
                    name: "DeployStaging",
                    actions: [{
                        name: "DeployStaging",
                        category: "Build",
                        owner: "AWS",
                        provider: "CodeBuild",
                        version: "1",
                        runOrder: 1,
                        configuration: {
                            ProjectName: stagingDeployment.name,
                        },
                        outputArtifacts: ["staging_output"],
                        inputArtifacts: ["source_output"],
                    }],
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
                            configuration: {},
                        },
                        {
                            name: "DeployProduction",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            runOrder: 2,
                            configuration: {
                                ProjectName: prodDeployment.name,
                            },
                            outputArtifacts: ["production_output"],
                            inputArtifacts: ["source_output"],
                        },
                    ],
                },
            ],
            triggers: [{
                providerType: "CodeStarSourceConnection",
                gitConfiguration: {
                    sourceActionName: "Github",
                    pushes: [{
                        branches: {
                            includes: [this.config.branchName!],
                        },
                    }],
                },
            }],
            name: `l-${this.config.appName}-pipeline`,
            roleArn: role.arn,
            tags: {
                Name: `l-${this.config.appName}-pipeline`,
                Component: `l-${this.config.appName}`,
            },
        });

        createLaunchPipelineNotificationRule(
            `l-${this.config.appName}-pipeline-status-notifications`,
            pipeline.arn,
            ["codepipeline-pipeline-manual-approval-succeeded"],
            slackAlertsTopicArn,
            {
                Name: `l-${this.config.appName}-pipeline-status-notifications`,
                Component: `l-${this.config.appName}`,
            },
            { parent: this },
        );

        createProdStageFailureAlert(
            `l-${this.config.appName}-prod-stage-failure`,
            pipeline.arn,
            slackAlertsTopicArn,
            {
                Name: `l-${this.config.appName}-prod-stage-failure`,
                Component: `l-${this.config.appName}`,
            },
            { parent: this },
        );
    }

    protected getBuildSpec(): string {
        const opsAccountId = BOUNDLESS_OPS_ACCOUNT_ID;
        return `
    version: 0.2

    env:
      git-credential-helper: yes

    phases:
      pre_build:
        commands:
          - set -e
          - curl -fsSL https://get.pulumi.com/ | sh -s -- --version 3.193.0
          - export PATH=$PATH:$HOME/.pulumi/bin
          - pulumi login --non-interactive "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
          - git submodule update --init --recursive
          - echo $DOCKER_PAT > docker_token.txt
          - cat docker_token.txt | docker login -u $DOCKER_USERNAME --password-stdin
          - echo $GITHUB_TOKEN | docker login ghcr.io -u boundless-xyz --password-stdin
      build:
        commands:
          - |
            set -e
            echo "Assuming deployment role $DEPLOYMENT_ROLE_ARN"
            ASSUMED_ROLE=$(aws sts assume-role --role-arn "$DEPLOYMENT_ROLE_ARN" --duration-seconds ${ASSUME_ROLE_CHAINED_MAX_SESSION_SECONDS} --role-session-name Deployment --output text | tail -1)
            export AWS_ACCESS_KEY_ID=$(echo "$ASSUMED_ROLE" | awk '{print $2}')
            export AWS_SECRET_ACCESS_KEY=$(echo "$ASSUMED_ROLE" | awk '{print $4}')
            export AWS_SESSION_TOKEN=$(echo "$ASSUMED_ROLE" | awk '{print $5}')
            CALLER=$(aws sts get-caller-identity --query Arn --output text)
            echo "Running as $CALLER"
            if echo "$CALLER" | grep -q "${opsAccountId}"; then
              echo "ERROR: Still using ops account (pipeline role). Assume-role did not take effect."
              exit 1
            fi
            if [ ! -d "infra/$APP_NAME" ]; then
              echo "ERROR: infra/$APP_NAME is missing from this checkout (branch: $(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown), commit: $(git rev-parse --short HEAD 2>/dev/null || echo unknown))."
              echo "Ensure infra/kailua-batch is committed on the pipeline source branch."
              echo "Top-level infra dirs:"
              ls -1 infra 2>/dev/null || ls -1
              exit 1
            fi
            cd "infra/$APP_NAME"
            pulumi install
            npm run build
            echo "DEPLOYING stack $STACK_NAME"
            pulumi stack select "$STACK_NAME"
            pulumi cancel --yes || true
            pulumi refresh --yes
            pulumi up --yes
            echo "pulumi up completed for stack $STACK_NAME"
    `;
    }

    protected codeBuildProjectArgs(
        appName: string,
        stackName: string,
        role: aws.iam.Role,
        serviceAccountRoleArn: string,
        dockerUsername: string,
        dockerTokenSecret: aws.secretsmanager.Secret,
        githubTokenSecret: aws.secretsmanager.Secret,
    ): aws.codebuild.ProjectArgs {
        return {
            buildTimeout: this.config.buildTimeout!,
            description: `Launch deployment for ${this.config.appName} (${stackName})`,
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
                        value: serviceAccountRoleArn,
                    },
                    {
                        name: "STACK_NAME",
                        type: "PLAINTEXT",
                        value: stackName,
                    },
                    {
                        name: "APP_NAME",
                        type: "PLAINTEXT",
                        value: appName,
                    },
                    {
                        name: "GITHUB_TOKEN",
                        type: "SECRETS_MANAGER",
                        value: githubTokenSecret.name,
                    },
                    {
                        name: "DOCKER_USERNAME",
                        type: "PLAINTEXT",
                        value: dockerUsername,
                    },
                    {
                        name: "DOCKER_PAT",
                        type: "SECRETS_MANAGER",
                        value: dockerTokenSecret.name,
                    },
                ],
            },
            artifacts: { type: "CODEPIPELINE" },
            source: {
                type: "CODEPIPELINE",
                buildspec: this.getBuildSpec(),
            },
            tags: {
                Name: `l-${this.config.appName}-deployment`,
                Component: `l-${this.config.appName}`,
            },
        };
    }
}
