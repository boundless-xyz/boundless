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
      - curl -fsSL https://get.pulumi.com/ | sh
      - export PATH=$PATH:$HOME/.pulumi/bin
      - pulumi login --non-interactive "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
      - git submodule update --init --recursive
      - cd infra/packer
      - wget https://releases.hashicorp.com/packer/1.9.4/packer_1.9.4_linux_amd64.zip
      - unzip packer_1.9.4_linux_amd64.zip
      - sudo mv packer /usr/local/bin/
      - packer version
  build:
    commands:
      - echo "Building AMI with Packer..."
      - cd infra/packer
      - packer build -var "aws_region=us-west-2" -var "boundless_version=$BOUNDLESS_VERSION" bento.pkr.hcl
  post_build:
    commands:
      - echo "AMI build completed successfully"
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

        // Custom policy for cross-account AMI sharing
        const packerCrossAccountPolicy = new aws.iam.Policy("packer-cross-account-policy", {
            policy: pulumi.all([serviceAccountIds.staging, serviceAccountIds.production]).apply(([stagingId, prodId]) => JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "ec2:ModifyImageAttribute",
                            "ec2:DescribeImages",
                            "ec2:DescribeImageAttribute"
                        ],
                        Resource: "*"
                    },
                    {
                        Effect: "Allow",
                        Action: [
                            "ec2:ModifyImageAttribute"
                        ],
                        Resource: "arn:aws:ec2:us-west-2:*:image/*",
                        Condition: {
                            StringEquals: {
                                "ec2:Attribute": "launchPermission"
                            }
                        }
                    }
                ]
            }))
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("packer-cross-account-policy-attachment", {
            role: packerRole.name,
            policyArn: packerCrossAccountPolicy.arn,
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
                        name: "BOUNDLESS_VERSION",
                        value: "v1.0.1",
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

        // Lambda function to share AMI with service accounts
        const amiSharingLambda = new aws.lambda.Function("ami-sharing-lambda", {
            name: `${APP_NAME}-ami-sharing`,
            runtime: "python3.9",
            handler: "index.handler",
            role: new aws.iam.Role("ami-sharing-lambda-role", {
                assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
                    Service: "lambda.amazonaws.com",
                }),
                inlinePolicies: [{
                    name: "ami-sharing-policy",
                    policy: pulumi.all([serviceAccountIds.staging, serviceAccountIds.production]).apply(([stagingId, prodId]) => JSON.stringify({
                        Version: "2012-10-17",
                        Statement: [
                            {
                                Effect: "Allow",
                                Action: [
                                    "logs:CreateLogGroup",
                                    "logs:CreateLogStream",
                                    "logs:PutLogEvents"
                                ],
                                Resource: "arn:aws:logs:*:*:*"
                            },
                            {
                                Effect: "Allow",
                                Action: [
                                    "ec2:ModifyImageAttribute",
                                    "ec2:DescribeImages"
                                ],
                                Resource: "*"
                            }
                        ]
                    }))
                }]
            }, { parent: this }).arn,
            code: new pulumi.asset.AssetArchive({
                "index.py": new pulumi.asset.StringAsset(`
import json
import boto3
import os

def handler(event, context):
    ec2 = boto3.client('ec2')

    # Get AMI ID from CodeBuild event
    ami_id = event.get('detail', {}).get('additional-information', {}).get('environment', {}).get('environment-variables', [])
    ami_id = next((var['value'] for var in ami_id if var['name'] == 'AMI_ID'), None)

    if not ami_id:
        print("No AMI ID found in event")
        return {"statusCode": 400, "body": "No AMI ID found"}

    # Service account IDs
    staging_account = "${serviceAccountIds.staging}"
    production_account = "${serviceAccountIds.production}"

    try:
        # Share AMI with staging account
        ec2.modify_image_attribute(
            ImageId=ami_id,
            Attribute='launchPermission',
            OperationType='add',
            UserIds=[staging_account]
        )
        print(f"Shared AMI {ami_id} with staging account {staging_account}")

        # Share AMI with production account
        ec2.modify_image_attribute(
            ImageId=ami_id,
            Attribute='launchPermission',
            OperationType='add',
            UserIds=[production_account]
        )
        print(f"Shared AMI {ami_id} with production account {production_account}")

        return {
            "statusCode": 200,
            "body": json.dumps({
                "message": f"AMI {ami_id} shared successfully",
                "staging_account": staging_account,
                "production_account": production_account
            })
        }
    except Exception as e:
        print(f"Error sharing AMI: {str(e)}")
        return {
            "statusCode": 500,
            "body": json.dumps({"error": str(e)})
        }
`)
            }),
        }, { parent: this });

        // EventBridge rule to trigger AMI sharing
        const amiSharingRule = new aws.cloudwatch.EventRule("ami-sharing-rule", {
            name: `${APP_NAME}-ami-sharing-rule`,
            description: "Trigger AMI sharing when Packer build completes",
            eventPattern: JSON.stringify({
                source: ["aws.codebuild"],
                "detail-type": ["CodeBuild Build State Change"],
                detail: {
                    "project-name": [packerBuildProject.name],
                    "build-status": ["SUCCEEDED"]
                }
            })
        }, { parent: this });

        new aws.cloudwatch.EventTarget("ami-sharing-target", {
            rule: amiSharingRule.name,
            targetId: "AmiSharingTarget",
            arn: amiSharingLambda.arn,
        }, { parent: this });

        new aws.lambda.Permission("ami-sharing-lambda-permission", {
            statementId: "AllowExecutionFromEventBridge",
            action: "lambda:InvokeFunction",
            function: amiSharingLambda.name,
            principal: "events.amazonaws.com",
            sourceArn: amiSharingRule.arn,
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
                },
                {
                    name: "ShareAMI",
                    actions: [{
                        name: "ShareAMI",
                        category: "Invoke",
                        owner: "AWS",
                        provider: "Lambda",
                        version: "1",
                        configuration: {
                            FunctionName: amiSharingLambda.name
                        },
                        inputArtifacts: ["ami_output"],
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
        this.amiSharingLambdaName = amiSharingLambda.name;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly packerBuildProjectName!: pulumi.Output<string>;
    public readonly amiSharingLambdaName!: pulumi.Output<string>;
}
