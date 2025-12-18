import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./base";

interface PackerPipelineArgs extends BasePipelineArgs {
    opsAccountId: string;
    serviceAccountIds: {
        development: string;
        staging: string;
        production: string;
    };
}

// The name of the app that we are deploying. Must match the name of the directory in the infra directory.
const APP_NAME = "packer";
// The branch that we should deploy from on push.
const BRANCH_NAME = "main";

// Buildspec for Packer AMI builds
const PACKER_BUILD_SPEC = `
version: 0.2

env:
  git-credential-helper: yes

phases:
  pre_build:
    commands:
      - wget https://releases.hashicorp.com/packer/1.14.2/packer_1.14.2_linux_amd64.zip
      - unzip packer_1.14.2_linux_amd64.zip
      - sudo mv packer /usr/local/bin/
      - packer version
  build:
    commands:
      - echo "Building AMI with Packer and sharing with service accounts..."
      - cd infra/$APP_NAME
      - echo "Initializing Packer plugins..."
      - packer init bento.pkr.hcl
      - echo "Building AMI with Packer..."
      - packer build -var "aws_region=us-west-2" -var "boundless_bento_version=$BOUNDLESS_BENTO_VERSION" -var "boundless_broker_version=$BOUNDLESS_BROKER_VERSION" -var "service_account_ids=[\"$DEVELOPMENT_ACCOUNT_ID\",\"$STAGING_ACCOUNT_ID\",\"$PRODUCTION_ACCOUNT_ID\"]" bento.pkr.hcl
  post_build:
    commands:
      - echo "AMI build and sharing completed successfully"
      - echo "AMI ID will be available in the build logs"
`;

export class PackerPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: PackerPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:packer-pipeline", name, args, opts);

        const { artifactBucket, connection, serviceAccountIds, role, slackAlertsTopicArn } = args;

        // CodeBuild project for Packer builds
        const packerBuildProject = new aws.codebuild.Project("packer-build-project", {
            name: `${APP_NAME}-packer-build`,
            serviceRole: role.arn,
            artifacts: {
                type: "CODEPIPELINE",
            },
            environment: {
                type: "LINUX_CONTAINER",
                image: "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                computeType: "BUILD_GENERAL1_LARGE",
                privilegedMode: true,
                environmentVariables: [
                    {
                        name: "APP_NAME",
                        value: APP_NAME,
                    },
                    {
                        name: "BOUNDLESS_BENTO_VERSION",
                        value: "v1.2.0",
                    },
                    {
                        name: "BOUNDLESS_BROKER_VERSION",
                        value: "v1.2.0",
                    },
                    {
                        name: "DEVELOPMENT_ACCOUNT_ID",
                        value: serviceAccountIds.development,
                    },
                    {
                        name: "STAGING_ACCOUNT_ID",
                        value: serviceAccountIds.staging,
                    },
                    {
                        name: "PRODUCTION_ACCOUNT_ID",
                        value: serviceAccountIds.production,
                    },
                    {
                        name: "AWS_DEFAULT_REGION",
                        value: "us-west-2",
                    },
                ],
            },
            source: {
                type: "CODEPIPELINE",
                buildspec: PACKER_BUILD_SPEC,
            },
            sourceVersion: "CODEPIPELINE",
            tags: {
                Name: `${APP_NAME}-packer-build`,
                Component: "packer",
            },
        }, { parent: this });


        // Create the main pipeline
        const pipeline = new aws.codepipeline.Pipeline(`${APP_NAME}-pipeline`, {
            roleArn: args.role.arn,
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
                    name: "BuildAMI",
                    actions: [{
                        name: "BuildAMI",
                        category: "Build",
                        owner: "AWS",
                        provider: "CodeBuild",
                        version: "1",
                        configuration: {
                            ProjectName: packerBuildProject.name
                        },
                        outputArtifacts: ["ami_output"],
                        inputArtifacts: ["source_output"],
                    }],
                }
            ],
            tags: {
                Name: `${APP_NAME}-pipeline`,
                Component: "packer",
            },
        }, { parent: this });

        // Create notification rule
        new aws.codestarnotifications.NotificationRule(`${APP_NAME}-pipeline-notifications`, {
            name: `${APP_NAME}-pipeline-notifications`,
            eventTypeIds: [
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
                Component: "packer",
            },
        });

        // Outputs
        this.pipelineName = pipeline.name;
        this.packerBuildProjectName = packerBuildProject.name;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly packerBuildProjectName!: pulumi.Output<string>;
}
