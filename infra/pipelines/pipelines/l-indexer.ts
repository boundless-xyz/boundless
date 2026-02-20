import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN } from "../accountConstants";
import { LaunchDefaultPipeline, LaunchPipelineConfig, BasePipelineArgs } from "./l-base";

interface LIndexerPipelineArgs extends BasePipelineArgs {
}

const config: LaunchPipelineConfig = {
    appName: "indexer",
    buildTimeout: 120,
    computeType: "BUILD_GENERAL1_LARGE",
    additionalBuildSpecCommands: [
        'curl https://sh.rustup.rs -sSf | sh -s -- -y',
        '. "$HOME/.cargo/env"',
        'curl -fsSL https://cargo-lambda.info/install.sh | sh -s -- -y',
        '. "$HOME/.cargo/env"',
        'npm install -g @ziglang/cli'
    ],
    branchName: "main",
};

export class LIndexerPipeline extends LaunchDefaultPipeline {
    constructor(name: string, args: LIndexerPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super(`boundless:pipelines:l-indexerPipeline`, name, config, args, opts);
    }

    protected createPipeline(args: BasePipelineArgs) {
        const { connection, artifactBucket, role, githubToken, dockerUsername, dockerToken, slackAlertsTopicArn } = args;

        const { githubTokenSecret, dockerTokenSecret } = this.createSecretsAndPolicy(role, githubToken, dockerToken);

        // Create CodeBuild projects for each stack
        const stagingDeploymentBaseSepolia = new aws.codebuild.Project(
            `l-${this.config.appName}-staging-84532-build`,
            this.codeBuildProjectArgs(this.config.appName, "l-staging-84532", role, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
            { dependsOn: [role] }
        );

        const stagingDeploymentEthSepolia = new aws.codebuild.Project(
            `l-${this.config.appName}-staging-11155111-build`,
            this.codeBuildProjectArgs(this.config.appName, "l-staging-11155111", role, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
            { dependsOn: [role] }
        );

        const prodDeploymentBaseSepolia = new aws.codebuild.Project(
            `l-${this.config.appName}-prod-84532-build`,
            this.codeBuildProjectArgs(this.config.appName, "l-prod-84532", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
            { dependsOn: [role] }
        );

        const prodDeploymentEthSepolia = new aws.codebuild.Project(
            `l-${this.config.appName}-prod-11155111-build`,
            this.codeBuildProjectArgs(this.config.appName, "l-prod-11155111", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
            { dependsOn: [role] }
        );

        const prodDeploymentBaseMainnet = new aws.codebuild.Project(
            `l-${this.config.appName}-prod-8453-build`,
            this.codeBuildProjectArgs(this.config.appName, "l-prod-8453", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
            { dependsOn: [role] }
        );

        const prodDeploymentEthMainnet = new aws.codebuild.Project(
            `l-${this.config.appName}-prod-1-build`,
            this.codeBuildProjectArgs(this.config.appName, "l-prod-1", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, dockerUsername, dockerTokenSecret, githubTokenSecret),
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
                            BranchName: this.config.branchName!,
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
                        },
                        {
                            name: "DeployStagingEthSepolia",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            runOrder: 1,
                            configuration: {
                                ProjectName: stagingDeploymentEthSepolia.name
                            },
                            outputArtifacts: ["staging_output_eth_sepolia"],
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
                        },
                        {
                            name: "DeployProductionEthMainnet",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            runOrder: 2,
                            configuration: {
                                ProjectName: prodDeploymentEthMainnet.name
                            },
                            outputArtifacts: ["production_output_eth_mainnet"],
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
                                    includes: [this.config.branchName!],
                                },
                            },
                        ],
                    },
                },
            ],
            name: `l-${this.config.appName}-pipeline`,
            roleArn: role.arn,
            tags: {
                Name: `l-${this.config.appName}-pipeline`,
                Component: `l-${this.config.appName}`,
            },
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
                    address: slackAlertsTopicArn.apply((arn: string) => arn),
                },
            ],
            tags: {
                Name: `l-${this.config.appName}-pipeline-notifications`,
                Component: `l-${this.config.appName}`,
            },
        });
    }
}