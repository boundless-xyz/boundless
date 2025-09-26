import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./base";

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
const BRANCH_NAME = "zeroecco/prover_cluster";

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
      - git submodule update --init --recursive
      - cd infra/prover-cluster
      - pulumi install
      - pulumi stack select $STACK_NAME
  build:
    commands:
      - echo "Deploying Prover Cluster to $ENVIRONMENT"
      - pulumi up --yes
`;

export class ProverClusterPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: ProverClusterPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:prover-cluster-pipeline", name, args, opts);

        const { artifactBucket, connection, stagingAccountId, productionAccountId, opsAccountId, amiId, boundlessBentoVersion, boundlessBrokerVersion, stagingStackName, stagingStackName2, productionStackName } = args;

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

        // Add S3 permissions for artifact bucket access
        new aws.iam.RolePolicy("prover-cluster-s3-artifacts-policy", {
            role: proverClusterRole.id,
            policy: pulumi.all([artifactBucket.arn]).apply(([bucketArn]) => JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "s3:GetObject",
                            "s3:GetObjectVersion",
                            "s3:PutObject",
                            "s3:PutObjectAcl",
                            "s3:ListBucket",
                        ],
                        Resource: [
                            bucketArn,
                            `${bucketArn}/*`,
                        ],
                    },
                ],
            })),
        }, { parent: this });

        // Add CodeStar connection permissions for GitHub access
        new aws.iam.RolePolicy("prover-cluster-codestar-connection-policy", {
            role: proverClusterRole.id,
            policy: pulumi.all([connection.arn]).apply(([connectionArn]) => JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "codestar-connections:UseConnection",
                        ],
                        Resource: connectionArn,
                    },
                ],
            })),
        }, { parent: this });

        // Add S3 permissions for Pulumi state bucket access
        new aws.iam.RolePolicy("prover-cluster-pulumi-state-policy", {
            role: proverClusterRole.id,
            policy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "s3:GetObject",
                            "s3:ListBucket",
                            "s3:PutObject",
                            "s3:DeleteObject",
                        ],
                        Resource: [
                            "arn:aws:s3:::boundless-pulumi-state",
                            "arn:aws:s3:::boundless-pulumi-state/*",
                        ],
                    },
                ],
            }),
        }, { parent: this });

        // Add KMS permissions for Pulumi state bucket encryption key access
        new aws.iam.RolePolicy("prover-cluster-pulumi-state-kms-policy", {
            role: proverClusterRole.id,
            policy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "kms:Encrypt",
                            "kms:Decrypt",
                            "kms:ReEncrypt*",
                            "kms:GenerateDataKey*",
                            "kms:DescribeKey",
                        ],
                        Resource: "*",
                    },
                ],
            }),
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
                        Resource: [
                            `arn:aws:iam::${stagingAccountId}:role/DeploymentRole`,
                            `arn:aws:iam::${productionAccountId}:role/DeploymentRole`
                        ]
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

        // Helper function to create CodeBuild projects
        const createCodeBuildProject = (environment: string, accountId: string, stackName: string) => {
            return new aws.codebuild.Project(`${APP_NAME}-${stackName}`, {
                name: `${APP_NAME}-${stackName}-deployment`,
                serviceRole: proverClusterRole.arn,
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
                            value: `arn:aws:iam::${accountId}:role/DeploymentRole`
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
                            name: "BOUNDLESS_BENTO_VERSION",
                            type: "PLAINTEXT",
                            value: boundlessBentoVersion || "v1.0.1"
                        },
                        {
                            name: "BOUNDLESS_BROKER_VERSION",
                            type: "PLAINTEXT",
                            value: boundlessBrokerVersion || "v1.0.0"
                        },
                        {
                            name: "AWS_DEFAULT_REGION",
                            type: "PLAINTEXT",
                            value: "us-west-2"
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
                    Project: "boundless",
                    Component: "prover-cluster",
                    Environment: environment,
                },
            }, { parent: this });
        };

        const stagingBuildBaseSepolia = createCodeBuildProject("staging", stagingAccountId, "staging-84532");
        const stagingBuildEthSepolia = createCodeBuildProject("staging", stagingAccountId, "staging-11155111");
        const productionBuildBaseMainnet = createCodeBuildProject("production", productionAccountId, "prod-8453");

        // Create the main pipeline with staging and production stages
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
                    name: "DeployStaging",
                    actions: [
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
                        },
                        {
                            name: "DeployStagingEthSepolia",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            configuration: {
                                ProjectName: stagingBuildEthSepolia.name
                            },
                            outputArtifacts: ["staging_output_eth_sepolia"],
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
                            name: "DeployProductionBaseMainnet",
                            category: "Build",
                            owner: "AWS",
                            provider: "CodeBuild",
                            version: "1",
                            runOrder: 2,
                            configuration: {
                                ProjectName: productionBuildBaseMainnet.name
                            },
                            outputArtifacts: ["production_output_base_mainnet"],
                            inputArtifacts: ["source_output"],
                        }
                    ]
                }
            ],
            tags: {
                Project: "boundless",
                Component: "prover-cluster",
                Environment: "multi",
            },
        }, { parent: this });

        // Outputs
        this.pipelineName = pipeline.name;
        this.stagingBuildProjectName = stagingBuildBaseSepolia.name;
        this.stagingBuildProject2Name = stagingBuildEthSepolia.name;
        this.productionBuildProjectName = productionBuildBaseMainnet.name;
        this.deploymentRoleArn = proverClusterRole.arn;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly stagingBuildProjectName!: pulumi.Output<string>;
    public readonly stagingBuildProject2Name!: pulumi.Output<string>;
    public readonly productionBuildProjectName!: pulumi.Output<string>;
    public readonly deploymentRoleArn!: pulumi.Output<string>;
}
