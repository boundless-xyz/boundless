import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

// Defines the shared resources used by all deployment pipelines like IAM roles and the
// S3 artifact bucket.
export class CodePipelineSharedResources extends pulumi.ComponentResource {
  public role: aws.iam.Role;
  public artifactBucket: aws.s3.Bucket;

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
  }
}
