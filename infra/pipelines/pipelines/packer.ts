import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./base";

interface PackerPipelineArgs extends BasePipelineArgs {
    opsAccountId: string;
    serviceAccountIds: {
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
      - echo "Starting Packer AMI build..."
      - echo Assuming role $DEPLOYMENT_ROLE_ARN
      - ASSUMED_ROLE=$(aws sts assume-role --role-arn $DEPLOYMENT_ROLE_ARN --role-session-name PackerBuild --output text | tail -1)
      - export AWS_ACCESS_KEY_ID=$(echo $ASSUMED_ROLE | awk '{print $2}')
      - export AWS_SECRET_ACCESS_KEY=$(echo $ASSUMED_ROLE | awk '{print $4}')
      - export AWS_SESSION_TOKEN=$(echo $ASSUMED_ROLE | awk '{print $5}')
      - cd infra/packer
      - wget https://releases.hashicorp.com/packer/1.9.4/packer_1.9.4_linux_amd64.zip
      - unzip packer_1.9.4_linux_amd64.zip
      - sudo mv packer /usr/local/bin/
      - packer version
  build:
    commands:
      - echo "Building AMI with Packer and sharing with service accounts..."
      - cd infra/packer
      - packer build -var "aws_region=us-west-2" -var "boundless_bento_version=$BOUNDLESS_BENTO_VERSION" -var "boundless_broker_version=$BOUNDLESS_BROKER_VERSION" -var "service_account_ids=[\"$STAGING_ACCOUNT_ID\",\"$PRODUCTION_ACCOUNT_ID\"]" bento.pkr.hcl
  post_build:
    commands:
      - echo "AMI build and sharing completed successfully"
      - echo "AMI ID will be available in the build logs"
`;

export class PackerPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: PackerPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:packer-pipeline", name, args, opts);

        const { artifactBucket, connection, opsAccountId, serviceAccountIds } = args;

        // Create IAM role for Packer builds in ops account
        const packerRole = new aws.iam.Role("packer-build-role", {
            assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
                Service: "codebuild.amazonaws.com",
            }),
        }, { parent: this });

        // Attach policies for Packer to create AMIs
        new aws.iam.RolePolicyAttachment("packer-ec2-policy", {
            role: packerRole.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonEC2FullAccess",
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("packer-ssm-policy", {
            role: packerRole.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonSSMFullAccess",
        }, { parent: this });

        // CodeBuild project for Packer builds
        const packerBuildProject = new aws.codebuild.Project("packer-build-project", {
            name: `${APP_NAME}-packer-build`,
            serviceRole: packerRole.arn,
            artifacts: {
                type: "NO_ARTIFACTS",
            },
            environment: {
                type: "LINUX_CONTAINER",
                image: "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                computeType: "BUILD_GENERAL1_LARGE",
                privilegedMode: true,
                environmentVariables: [
                    {
                        name: "DEPLOYMENT_ROLE_ARN",
                        value: `arn:aws:iam::${opsAccountId}:role/DeploymentRole`,
                    },
                    {
                        name: "BOUNDLESS_BENTO_VERSION",
                        value: "v1.0.1",
                    },
                    {
                        name: "BOUNDLESS_BROKER_VERSION",
                        value: "v1.0.0",
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
            tags: {
                Project: "boundless",
                Component: "packer",
                Environment: "ops",
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
                Project: "boundless",
                Component: "packer",
                Environment: "ops",
            },
        }, { parent: this });

        // Outputs
        this.pipelineName = pipeline.name;
        this.packerBuildProjectName = packerBuildProject.name;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly packerBuildProjectName!: pulumi.Output<string>;
}
