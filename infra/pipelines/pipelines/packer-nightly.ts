import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./base";

interface PackerNightlyPipelineArgs extends BasePipelineArgs {
    opsAccountId: string;
    serviceAccountIds: {
        development: string;
        staging: string;
        production: string;
    };
}

// The name of the app that we are deploying. Must match the name of the directory in the infra directory.
const APP_NAME = "nightly-build";
// The branch that we should deploy from on push.
const BRANCH_NAME = "main";

// Buildspec for nightly builds
const NIGHTLY_BUILD_SPEC = `
version: 0.2

env:
  git-credential-helper: yes

phases:
  pre_build:
    commands:
      - echo "Starting nightly build process..."
      - echo "Build started on $(date)"
      - echo "Building from branch $CODEBUILD_WEBHOOK_HEAD_REF"
      - echo "Commit $CODEBUILD_RESOLVED_SOURCE_VERSION"
      - sudo yum install -y git wget unzip jq
      - wget https://releases.hashicorp.com/packer/1.14.2/packer_1.14.2_linux_amd64.zip
      - unzip packer_1.14.2_linux_amd64.zip
      - sudo mv packer /usr/local/bin/
      - packer version

  build:
    commands:
      - echo "Building nightly artifacts..."
      - cd infra/packer
      - packer init bento_nightly.pkr.hcl
      - AWS_POLLING_MAX_ATTEMPTS=3600 AWS_POLLING_DELAY_SECONDS=30 packer build -var "service_account_ids=[\"$DEVELOPMENT_ACCOUNT_ID\",\"$STAGING_ACCOUNT_ID\",\"$PRODUCTION_ACCOUNT_ID\"]" bento_nightly.pkr.hcl
      - echo "Generating build artifacts..."
      - mkdir -p build-artifacts
      - echo "nightly-$(date +%Y%m%d-%H%M%S)" > build-artifacts/version.txt
      - echo "$CODEBUILD_RESOLVED_SOURCE_VERSION" > build-artifacts/commit.txt
      - echo "$CODEBUILD_WEBHOOK_HEAD_REF" > build-artifacts/branch.txt
      - echo "Creating build summary..."
      - jq -n --arg version "nightly-$(date +%Y%m%d-%H%M%S)" --arg commit "$CODEBUILD_RESOLVED_SOURCE_VERSION" --arg branch "$CODEBUILD_WEBHOOK_HEAD_REF" --arg buildTime "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg buildId "$CODEBUILD_BUILD_ID" '{version:$version, commit:$commit, branch:$branch, buildTime:$buildTime, buildId:$buildId, status:"success"}' > build-artifacts/build-summary.json

  post_build:
    commands:
      - echo "Nightly build completed successfully"
      - echo "Build artifacts"
      - ls -la build-artifacts/
      - cat build-artifacts/build-summary.json
      - echo "Build completed on $(date)"
`;

export class PackerNightlyPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: PackerNightlyPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:packer-nightly-pipeline", name, args, opts);

        const {artifactBucket, connection, serviceAccountIds, role, slackAlertsTopicArn} = args;

        // CodeBuild project for nightly builds
        const nightlyBuildProject = new aws.codebuild.Project("packer-nightly-build-project", {
            name: `${APP_NAME}-packer-nightly-build`,
            serviceRole: role.arn,
            buildTimeout: 480, // 8 hours (480 minutes)
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
                    {
                        name: "RUST_BACKTRACE",
                        value: "1",
                    },
                ],
            },
            source: {
                type: "CODEPIPELINE",
                buildspec: NIGHTLY_BUILD_SPEC,
            },
            sourceVersion: "CODEPIPELINE",
            tags: {
                Name: `${APP_NAME}-packer-nightly-build`,
                Component: "packer-nightly-build",
            },
        }, {parent: this});

        // Create the main pipeline
        const pipeline = new aws.codepipeline.Pipeline(`packer-nightly-pipeline`, {
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
                    name: "Build",
                    actions: [{
                        name: "Build",
                        category: "Build",
                        owner: "AWS",
                        provider: "CodeBuild",
                        version: "1",
                        configuration: {
                            ProjectName: nightlyBuildProject.name
                        },
                        outputArtifacts: ["build_output"],
                        inputArtifacts: ["source_output"],
                    }],
                }
            ],
            tags: {
                Name: "packer-nightly-pipeline",
                Component: "packer-nightly-build",
            },
        }, {parent: this});

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
        }, {parent: this});

        // Grant EventBridge permission to start pipeline execution
        new aws.iam.RolePolicy(`${APP_NAME}-eventbridge-policy`, {
            role: eventBridgeRole.id,
            policy: pipeline.arn.apply((pipelineArn: string) =>
                JSON.stringify({
                    Version: "2012-10-17",
                    Statement: [{
                        Effect: "Allow",
                        Action: "codepipeline:StartPipelineExecution",
                        Resource: pipelineArn
                    }]
                })
            )
        }, {parent: this});

        // EventBridge rule for nightly builds (runs at 2 AM UTC daily)
        const nightlyScheduleRule = new aws.cloudwatch.EventRule("packer-nightly-schedule-rule", {
            name: "boundless-nightly-build-schedule",
            description: "Trigger nightly builds at 2 AM UTC",
            scheduleExpression: "cron(0 2 * * ? *)", // 2 AM UTC daily
            state: "ENABLED",
            tags: {
                Name: "packer-nightly-schedule-rule",
                Component: "packer-nightly-build",
            },
        }, {parent: this});

        // EventBridge target to start the pipeline
        new aws.cloudwatch.EventTarget("packer-nightly-schedule-target", {
            rule: nightlyScheduleRule.name,
            arn: pipeline.arn,
            roleArn: eventBridgeRole.arn,
        }, {parent: this});

        // Create notification rule
        new aws.codestarnotifications.NotificationRule(`packer-nightly-pipeline-notifications`, {
            name: `packer-nightly-pipeline-notifications`,
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
                Name: `packer-nightly-pipeline-notifications`,
                Component: "packer",
            },
        });

        // Outputs
        this.pipelineName = pipeline.name;
        this.buildProjectName = nightlyBuildProject.name;
        this.scheduleRuleName = nightlyScheduleRule.name;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly buildProjectName!: pulumi.Output<string>;
    public readonly scheduleRuleName!: pulumi.Output<string>;
}
