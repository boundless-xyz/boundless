import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./base";

interface ProverClusterPipelineArgs extends BasePipelineArgs {
    environment: "staging" | "production";
    serviceAccountId: string;
    opsAccountId: string;
    amiId?: string;
}

// The name of the app that we are deploying. Must match the name of the directory in the infra directory.
const APP_NAME = "prover-cluster";
// The branch that we should deploy from on push.
const BRANCH_NAME = "main";

// Buildspec for prover cluster deployment
const PROVER_CLUSTER_BUILD_SPEC = `
version: 0.2

env:
  git-credential-helper: yes

phases:
  pre_build:
    commands:
      - echo "Starting Prover Cluster deployment..."
      - echo Assuming role $DEPLOYMENT_ROLE_ARN
      - ASSUMED_ROLE=$(aws sts assume-role --role-arn $DEPLOYMENT_ROLE_ARN --role-session-name ProverClusterDeployment --output text | tail -1)
      - export AWS_ACCESS_KEY_ID=$(echo $ASSUMED_ROLE | awk '{print $2}')
      - export AWS_SECRET_ACCESS_KEY=$(echo $ASSUMED_ROLE | awk '{print $4}')
      - export AWS_SESSION_TOKEN=$(echo $ASSUMED_ROLE | awk '{print $5}')
      - curl -fsSL https://get.pulumi.com/ | sh
      - export PATH=$PATH:$HOME/.pulumi/bin
      - pulumi login --non-interactive "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
      - git submodule update --init --recursive
      - cd infra/prover-cluster
      - pulumi install
      - echo "DEPLOYING stack $STACK_NAME"
      - pulumi stack select $STACK_NAME
  build:
    commands:
      - echo "Deploying Prover Cluster to $ENVIRONMENT..."
      - pulumi up --yes
  post_build:
    commands:
      - echo "Prover Cluster deployment completed successfully"
      - echo "Cluster endpoints:"
      - pulumi stack output --json
`;

export class ProverClusterPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: ProverClusterPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:prover-cluster-pipeline", name, args, opts);

        const { artifactBucket, connection, environment, serviceAccountId, opsAccountId, amiId } = args;

        // Create IAM role for prover cluster deployment
        const proverClusterRole = new aws.iam.Role("prover-cluster-deployment-role", {
            assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
                Service: "codebuild.amazonaws.com",
            }),
        }, { parent: this });

        // Attach policies for prover cluster deployment
        new aws.iam.RolePolicyAttachment("prover-cluster-ec2-policy", {
            role: proverClusterRole.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonEC2FullAccess",
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("prover-cluster-iam-policy", {
            role: proverClusterRole.name,
            policyArn: "arn:aws:iam::aws:policy/IAMFullAccess",
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("prover-cluster-vpc-policy", {
            role: proverClusterRole.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonVPCFullAccess",
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("prover-cluster-autoscaling-policy", {
            role: proverClusterRole.name,
            policyArn: "arn:aws:iam::aws:policy/AutoScalingFullAccess",
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("prover-cluster-ssm-policy", {
            role: proverClusterRole.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonSSMFullAccess",
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("prover-cluster-cloudwatch-policy", {
            role: proverClusterRole.name,
            policyArn: "arn:aws:iam::aws:policy/CloudWatchFullAccess",
        }, { parent: this });

        // Custom policy for cross-account deployment
        const proverClusterCrossAccountPolicy = new aws.iam.Policy("prover-cluster-cross-account-policy", {
            policy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "sts:AssumeRole"
                        ],
                        Resource: `arn:aws:iam::${serviceAccountId}:role/DeploymentRole`
                    },
                    {
                        Effect: "Allow",
                        Action: [
                            "ec2:DescribeImages",
                            "ec2:DescribeInstances",
                            "ec2:DescribeSecurityGroups",
                            "ec2:DescribeSubnets",
                            "ec2:DescribeVpcs"
                        ],
                        Resource: "*"
                    }
                ]
            })
        }, { parent: this });

        new aws.iam.RolePolicyAttachment("prover-cluster-cross-account-policy-attachment", {
            role: proverClusterRole.name,
            policyArn: proverClusterCrossAccountPolicy.arn,
        }, { parent: this });

        // CodeBuild project for prover cluster deployment
        const proverClusterBuildProject = new aws.codebuild.Project("prover-cluster-build-project", {
            name: `${APP_NAME}-${environment}-deployment`,
            serviceRole: proverClusterRole.arn,
            artifacts: {
                type: "NO_ARTIFACTS",
            },
            environment: {
                type: "LINUX_CONTAINER",
                image: "aws/codebuild/amazonlinux2-x86_64-standard:5.0",
                computeType: "BUILD_GENERAL1_MEDIUM",
                environmentVariables: [
                    {
                        name: "DEPLOYMENT_ROLE_ARN",
                        value: `arn:aws:iam::${serviceAccountId}:role/DeploymentRole`,
                    },
                    {
                        name: "STACK_NAME",
                        value: `prover-cluster-${environment}`,
                    },
                    {
                        name: "ENVIRONMENT",
                        value: environment,
                    },
                    {
                        name: "SERVICE_ACCOUNT_ID",
                        value: serviceAccountId,
                    },
                    {
                        name: "AWS_DEFAULT_REGION",
                        value: "us-west-2",
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
            tags: {
                Project: "boundless",
                Component: "prover-cluster",
                Environment: environment,
            },
        }, { parent: this });

        // Create the main pipeline
        const pipeline = new aws.codepipeline.Pipeline(`${APP_NAME}-${environment}-pipeline`, {
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
                    name: "DeployCluster",
                    actions: [{
                        name: "DeployCluster",
                        category: "Build",
                        owner: "AWS",
                        provider: "CodeBuild",
                        version: "1",
                        configuration: {
                            ProjectName: proverClusterBuildProject.name
                        },
                        outputArtifacts: ["deployment_output"],
                        inputArtifacts: ["source_output"],
                    }],
                }
            ],
            tags: {
                Project: "boundless",
                Component: "prover-cluster",
                Environment: environment,
            },
        }, { parent: this });

        // Outputs
        this.pipelineName = pipeline.name;
        this.buildProjectName = proverClusterBuildProject.name;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly buildProjectName!: pulumi.Output<string>;
}
