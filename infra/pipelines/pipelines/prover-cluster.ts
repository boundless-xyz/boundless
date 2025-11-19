import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./base";
import { BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN } from "../accountConstants";

interface ProverClusterPipelineArgs extends BasePipelineArgs {
    stagingAccountId: string;
    productionAccountId: string;
    opsAccountId: string;
    amiId?: string;
    boundlessBentoVersion?: string;
    boundlessBrokerVersion?: string;
    // Stack name mappings for different environments
    stagingStackName?: string;
    stagingStackName2?: string;
    productionStackName?: string;
}

// The name of the app that we are deploying. Must match the name of the directory in the infra directory.
const APP_NAME = "prover-cluster";
// The branch that we should deploy from on push.
const BRANCH_NAME = "main";
// Buildspec for prover cluster deployment
const PROVER_CLUSTER_BUILD_SPEC = `version: 0.2

env:
  git-credential-helper: yes

phases:
  pre_build:
    commands:
      - ASSUMED_ROLE=$(aws sts assume-role --role-arn $DEPLOYMENT_ROLE_ARN --role-session-name ProverClusterDeployment --output text | tail -1)
      - export AWS_ACCESS_KEY_ID=$(echo $ASSUMED_ROLE | awk '{print $2}')
      - export AWS_SECRET_ACCESS_KEY=$(echo $ASSUMED_ROLE | awk '{print $4}')
      - export AWS_SESSION_TOKEN=$(echo $ASSUMED_ROLE | awk '{print $5}')
      - curl -fsSL https://get.pulumi.com/ | sh
      - export PATH=$PATH:$HOME/.pulumi/bin
      - pulumi login --non-interactive "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
      - cd infra/prover-cluster
      - pulumi install
      - pulumi stack select $STACK_NAME
  build:
    commands:
      - echo "Deploying Prover Cluster to $ENVIRONMENT"
      - pulumi refresh --yes
      - pulumi up --yes
`;

export class ProverClusterPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: ProverClusterPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:prover-cluster-pipeline", name, args, opts);

        const {
            artifactBucket,
            connection,
            role,
            stagingAccountId,
            productionAccountId,
            amiId,
            githubToken,
            slackAlertsTopicArn
        } = args;
        // Helper function to create CodeBuild projects
        const createCodeBuildProject = (environment: string, accountId: string, stackName: string) => {
            return new aws.codebuild.Project(`${APP_NAME}-${stackName}`, {
                name: `${APP_NAME}-${stackName}-deployment`,
                serviceRole: role.arn,
                artifacts: {
                    type: "CODEPIPELINE",
                },
                environment: {
                    type: "LINUX_CONTAINER",
                    image: "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                    computeType: "BUILD_GENERAL1_MEDIUM",
                    environmentVariables: [
                        {
                            name: "DEPLOYMENT_ROLE_ARN",
                            type: "PLAINTEXT",
                            value: accountId === stagingAccountId
                                ? BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN
                                : BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN
                        },
                        {
                            name: "STACK_NAME",
                            type: "PLAINTEXT",
                            value: stackName
                        },
                        {
                            name: "ENVIRONMENT",
                            type: "PLAINTEXT",
                            value: environment
                        },
                        {
                            name: "SERVICE_ACCOUNT_ID",
                            type: "PLAINTEXT",
                            value: accountId
                        },
                        {
                            name: "AWS_DEFAULT_REGION",
                            type: "PLAINTEXT",
                            value: "us-west-2"
                        },
                        {
                            name: "GITHUB_TOKEN",
                            type: "PLAINTEXT",
                            value: githubToken
                        },
                        ...(amiId ? [{
                            name: "AMI_ID",
                            value: amiId,
                        }] : []),
                    ],
                },
                source: {
                    type: "CODEPIPELINE",
                    buildspec: PROVER_CLUSTER_BUILD_SPEC,
                },
                sourceVersion: "CODEPIPELINE",
                tags: {
                    Name: `${APP_NAME}-${stackName}-deployment`,
                    Component: "prover-cluster",
                },
            }, { parent: this });
        };

        const stagingBuildBaseSepoliaNightly = createCodeBuildProject("staging", stagingAccountId, "staging-nightly-84532");
        const stagingBuildBaseSepolia = createCodeBuildProject("staging", stagingAccountId, "staging-84532");

        const productionBuildBaseMainnetNightly = createCodeBuildProject("production", productionAccountId, "prod-nightly-8453");
        const productionBuildBaseMainnetRelease = createCodeBuildProject("production", productionAccountId, "prod-release-8453");

        // Create the main pipeline with staging and production stages
        const pipeline = new aws.codepipeline.Pipeline(`${APP_NAME}-pipeline`, {
            roleArn: role.arn,
            pipelineType: "V2",
            artifactStores: [{
                type: "S3",
                location: artifactBucket.bucket
            }],
            stages: [
                {
                    name: "Source",
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
                            BranchName: BRANCH_NAME,
                            OutputArtifactFormat: "CODEBUILD_CLONE_REF"
                        },
                    }],
                },
                {
                    name: "DeployStaging",
                    actions: [
                        {
                            name: "DeployStagingBaseSepoliaNightly",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            configuration: {
                                ProjectName: stagingBuildBaseSepoliaNightly.name
                            },
                            outputArtifacts: ["staging_output_base_sepolia_nightly"],
                            inputArtifacts: ["source_output"],
                        },
                        {
                            name: "DeployStagingBaseSepolia",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            configuration: {
                                ProjectName: stagingBuildBaseSepolia.name
                            },
                            outputArtifacts: ["staging_output_base_sepolia"],
                            inputArtifacts: ["source_output"],
                        }
                    ],
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
                            name: "DeployProductionBaseMainnetNightly",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            runOrder: 2,
                            configuration: {
                                ProjectName: productionBuildBaseMainnetNightly.name
                            },
                            outputArtifacts: ["production_output_base_mainnet_nightly"],
                            inputArtifacts: ["source_output"],
                        },
                        {
                            name: "DeployProductionBaseMainnetRelease",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            runOrder: 2,
                            configuration: {
                                ProjectName: productionBuildBaseMainnetRelease.name
                            },
                            outputArtifacts: ["production_output_base_mainnet_release"],
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
                                    includes: [BRANCH_NAME],
                                },
                            },
                        ],
                    },
                },
            ],
            tags: {
                Name: `${APP_NAME}-pipeline`,
                Component: "prover-cluster",
            },
        }, { parent: this });

        // Create IAM role for EventBridge to execute the pipeline
        const eventBridgeRole = new aws.iam.Role(`${APP_NAME}-eventbridge-role`, {
            assumeRolePolicy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [{
                    Effect: "Allow",
                    Principal: {
                        Service: "events.amazonaws.com"
                    },
                    Action: "sts:AssumeRole"
                }]
            }),
        }, { parent: this });

        // Grant EventBridge permission to start pipeline execution
        new aws.iam.RolePolicy(`${APP_NAME}-eventbridge-policy`, {
            role: eventBridgeRole.id,
            policy: pulumi.all([pipeline.arn]).apply(([pipelineArn]) =>
                JSON.stringify({
                    Version: "2012-10-17",
                    Statement: [{
                        Effect: "Allow",
                        Action: "codepipeline:StartPipelineExecution",
                        Resource: pipelineArn
                    }]
                })
            )
        }, { parent: this });

        // Create EventBridge rule for daily 5am Central Time trigger
        // 10:00 UTC = 5am CDT (Central Daylight Time, UTC-5) / 4am CST (Central Standard Time, UTC-6)
        const dailyTrigger = new aws.cloudwatch.EventRule(`${APP_NAME}-daily-trigger`, {
            description: "Trigger prover-cluster pipeline daily at 5am Central Time (10:00 UTC)",
            scheduleExpression: "cron(0 10 * * ? *)",
        }, { parent: this });

        // Add pipeline as target for the EventBridge rule
        new aws.cloudwatch.EventTarget(`${APP_NAME}-daily-trigger-target`, {
            rule: dailyTrigger.name,
            arn: pipeline.arn,
            roleArn: eventBridgeRole.arn,
        }, { parent: this });

        // Create notification rule
        new aws.codestarnotifications.NotificationRule(`${APP_NAME}-pipeline-notifications`, {
            name: `${APP_NAME}-pipeline-notifications`,
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
            tags: {
                Name: `${APP_NAME}-pipeline-notifications`,
                Component: "prover-cluster",
            },
        });

        // Outputs
        this.pipelineName = pipeline.name;
        this.stagingBuildProjectName = stagingBuildBaseSepoliaNightly.name;
        this.productionBuildReleaseName = productionBuildBaseMainnetRelease.name;
        this.productionBuildNightlyName = productionBuildBaseMainnetNightly.name;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly stagingBuildProjectName!: pulumi.Output<string>;
    public readonly productionBuildReleaseName!: pulumi.Output<string>;
    public readonly productionBuildNightlyName!: pulumi.Output<string>;
}
