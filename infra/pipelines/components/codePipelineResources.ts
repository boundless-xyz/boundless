import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

// Defines the shared resources used by all deployment pipelines like IAM roles and the
// S3 artifact bucket.
export class CodePipelineSharedResources extends pulumi.ComponentResource {
  public role: aws.iam.Role;
  public artifactBucket: aws.s3.Bucket;
  public sccacheBucket: aws.s3.BucketV2;

  constructor(
    name: string,
    args: {
      accountId: string;
      serviceAccountDeploymentRoleArns: string[];
    },
    opts?: pulumi.ComponentResourceOptions
  ) {
    super('pipelines:CodePipelineRole', name, args, opts);

    // Defines the IAM role that CodeBuild and CodePipeline use to deploy the app.
    // This role can only be assumed by CodeBuild and CodePipeline services.
    this.role = new aws.iam.Role(`pipeline-role`, {
      assumeRolePolicy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [
          {
            Action: "sts:AssumeRole",
            Principal: {
              Service: "codebuild.amazonaws.com"
            },
            Effect: "Allow",
            Sid: ""
          },
          {
            Action: "sts:AssumeRole",
            Principal: {
              Service: "codepipeline.amazonaws.com"
            },
            Effect: "Allow",
            Sid: ""
          }
        ]
      })
    });

    // Defines the S3 bucket used to store the artifacts for all deployment pipelines.
    this.artifactBucket = new aws.s3.Bucket(`pipeline-artifacts`);

    // Defines the S3 bucket used for shared sccache build artifacts.
    this.sccacheBucket = new aws.s3.BucketV2(`boundlessSccacheBucket`, {
      bucket: "boundless-sccache",
    });

    new aws.s3.BucketServerSideEncryptionConfigurationV2(`sccacheBucketSSEConfiguration`, {
      bucket: this.sccacheBucket.id,
      rules: [
        {
          applyServerSideEncryptionByDefault: {
            sseAlgorithm: "AES256",
          },
        },
      ],
    });

    new aws.s3.BucketLifecycleConfigurationV2(`sccacheBucketLifecycle`, {
      bucket: this.sccacheBucket.id,
      rules: [
        {
          id: "expire-old-cache",
          status: "Enabled",
          filter: {},
          expiration: {
            days: 30,
          },
        },
      ],
    });

    // Defines the IAM policy that allows the CodeBuild and CodePipeline roles to access the artifact bucket.
    const s3AccessPolicy = new aws.iam.Policy(`pipeline-artifact-bucket-access`, {
      name: `pipeline-artifact-bucket-access`,
      policy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [{
          Action: ["s3:GetObject", "s3:GetObjectVersion", "s3:ListBucket", "s3:PutObject"],
          Effect: "Allow",
          Resource: [
            pulumi.interpolate`${this.artifactBucket.arn}`,
            pulumi.interpolate`${this.artifactBucket.arn}/*`
          ],
        }],
      }),
    });

    new aws.iam.RolePolicyAttachment(`pipeline-artifact-bucket-access-attachment`, {
      role: this.role,
      policyArn: s3AccessPolicy.arn,
    });

    // Defines the IAM policy that allows the role to access the CodeBuild service.
    new aws.iam.RolePolicyAttachment(`pipeline-codebuild-access`, {
      role: this.role,
      policyArn: aws.iam.ManagedPolicies.AWSCodeBuildDeveloperAccess
    });

    new aws.iam.RolePolicyAttachment(`pipeline-cloudwatch-access`, {
      role: this.role,
      policyArn: aws.iam.ManagedPolicies.CloudWatchFullAccessV2
    });

    // Add EC2 and SSM permissions for Packer builds
    new aws.iam.RolePolicyAttachment(`pipeline-ec2-access`, {
      role: this.role,
      policyArn: aws.iam.ManagedPolicies.AmazonEC2FullAccess
    });

    new aws.iam.RolePolicyAttachment(`pipeline-ssm-access`, {
      role: this.role,
      policyArn: aws.iam.ManagedPolicies.AmazonSSMFullAccess
    });

    // Defines the IAM policy that allows the role to access the CodeStar connection service. This
    // is used to connect to the Github repo for the app.
    const codeConnectionPolicy = new aws.iam.Policy(`pipeline-codeconnection-access-policy`, {
      name: `pipeline-codeconnection-access-policy`,
      policy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [{
          Action: [
            "codestar-connections:UseConnection",
            "codeconnections:UseConnection"
          ],
          Effect: "Allow",
          Resource: [
            "arn:aws:codestar-connections:*:*:connection/*",
            "arn:aws:codeconnections:*:*:connection/*"
          ]
        }],
      }),
    });

    new aws.iam.RolePolicyAttachment(`pipeline-codeconnection-access`, {
      role: this.role,
      policyArn: codeConnectionPolicy.arn
    });

    // Defines the IAM policy that allows the role to assume the deployment roles for the given
    // accounts. This is used to deploy the app cross-account to the service accounts.
    const serviceAccountDeploymentRoleAccessPolicy = new aws.iam.Policy(`pipeline-service-account-deployment-role-access`, {
      name: `pipeline-service-account-deployment-role-access`,
      policy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [{
          Action: ["sts:AssumeRole"],
          Effect: "Allow",
          Resource: args.serviceAccountDeploymentRoleArns,
        }],
      }),
    });

    new aws.iam.RolePolicyAttachment(`pipeline-service-account-deployment-role-access`, {
      role: this.role,
      policyArn: serviceAccountDeploymentRoleAccessPolicy.arn
    });

    const sccacheBucketPolicy = pulumi
      .all([this.sccacheBucket.arn, this.role.arn])
      .apply(([sccacheBucketArn, pipelineRoleArn]) =>
        pulumi.jsonStringify({
          Version: "2012-10-17",
          Statement: [
            {
              Effect: "Allow",
              Principal: {
                AWS: [pipelineRoleArn, ...args.serviceAccountDeploymentRoleArns],
              },
              Action: [
                "s3:GetObject",
                "s3:GetObjectAttributes",
                "s3:ListBucket",
                "s3:PutObject",
                "s3:DeleteObject",
              ],
              Resource: [sccacheBucketArn, `${sccacheBucketArn}/*`],
            },
          ],
        })
      );

    new aws.s3.BucketPolicy(`sccacheBucketPolicy`, {
      bucket: this.sccacheBucket.id,
      policy: sccacheBucketPolicy,
    });

    // Allow the pipeline role to read and manage ECR, IAM, ECS, RDS, and Lambda in the ops account.
    // Required when Pulumi stacks (e.g. indexer l-staging-84532) run with pipeline credentials.
    // Note: Resources in other accounts (e.g. ECS in staging) require the build to assume that
    // account's deployment role before running Pulumi.
    const pipelineOpsEcrandIamPolicy = new aws.iam.Policy(`pipeline-ops-ecr-iam-access`, {
      name: `pipeline-ops-ecr-iam-access`,
      policy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [
          {
            Sid: "ECRAccess",
            Effect: "Allow",
            Action: ["ecr:GetAuthorizationToken"],
            Resource: "*",
          },
          {
            Sid: "ECRRepositoryAccess",
            Effect: "Allow",
            Action: [
              "ecr:BatchCheckLayerAvailability",
              "ecr:BatchGetImage",
              "ecr:DescribeImages",
              "ecr:DescribeRepositories",
              "ecr:GetLifecyclePolicy",
              "ecr:GetRepositoryPolicy",
              "ecr:ListImages",
              "ecr:PutLifecyclePolicy",
              "ecr:PutImageScanningConfiguration",
              "ecr:PutImageTagMutability",
              "ecr:TagResource",
              "ecr:UntagResource",
            ],
            Resource: pulumi.interpolate`arn:aws:ecr:*:${args.accountId}:repository/*`,
          },
          {
            Sid: "IAMRolePolicyAccess",
            Effect: "Allow",
            Action: [
              "iam:GetRole",
              "iam:GetRolePolicy",
              "iam:ListRolePolicies",
              "iam:ListAttachedRolePolicies",
              "iam:ListRoleTags",
              "iam:PutRolePolicy",
              "iam:DeleteRolePolicy",
              "iam:AttachRolePolicy",
              "iam:DetachRolePolicy",
              "iam:PassRole",
              "iam:CreateRole",
              "iam:DeleteRole",
              "iam:TagRole",
              "iam:UntagRole",
              "iam:UpdateRole",
              "iam:UpdateRoleDescription",
            ],
            Resource: pulumi.interpolate`arn:aws:iam::${args.accountId}:role/*`,
          },
          {
            Sid: "ECSAccess",
            Effect: "Allow",
            Action: [
              "ecs:DescribeClusters",
              "ecs:DescribeServices",
              "ecs:DescribeTaskDefinition",
              "ecs:DescribeTasks",
              "ecs:ListClusters",
              "ecs:ListServices",
              "ecs:ListTaskDefinitions",
              "ecs:ListTasks",
              "ecs:CreateCluster",
              "ecs:DeleteCluster",
              "ecs:CreateService",
              "ecs:UpdateService",
              "ecs:DeleteService",
              "ecs:RegisterTaskDefinition",
              "ecs:DeregisterTaskDefinition",
              "ecs:TagResource",
              "ecs:UntagResource",
            ],
            Resource: "*",
          },
          {
            Sid: "RDSAccess",
            Effect: "Allow",
            Action: [
              "rds:DescribeDBInstances",
              "rds:DescribeDBClusters",
              "rds:DescribeDBClusterSnapshots",
              "rds:DescribeDBSnapshots",
              "rds:DescribeDBSubnetGroups",
              "rds:DescribeDBClusterParameterGroups",
              "rds:DescribeDBParameters",
            ],
            Resource: "*",
          },
          {
            Sid: "LambdaAccess",
            Effect: "Allow",
            Action: [
              "lambda:GetPolicy",
              "lambda:GetFunction",
              "lambda:GetFunctionUrlConfig",
              "lambda:ListVersionsByFunction",
              "lambda:ListAliases",
              "lambda:ListTags",
              "lambda:GetEventSourceMapping",
              "lambda:ListEventSourceMappings",
            ],
            Resource: pulumi.interpolate`arn:aws:lambda:*:${args.accountId}:function:*`,
          },
          {
            Sid: "WAFv2Access",
            Effect: "Allow",
            Action: [
              "wafv2:GetWebACL",
              "wafv2:GetWebACLForResource",
              "wafv2:ListWebACLs",
              "wafv2:ListResourcesForWebACL",
              "wafv2:AssociateWebACL",
              "wafv2:DisassociateWebACL",
            ],
            Resource: "*",
          },
        ],
      }),
    });

    new aws.iam.RolePolicyAttachment(`pipeline-ops-ecr-iam-access-attachment`, {
      role: this.role,
      policyArn: pipelineOpsEcrandIamPolicy.arn,
    });
  }
}
