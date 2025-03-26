import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

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

    this.artifactBucket = new aws.s3.Bucket(`pipeline-artifacts`);

    const s3AccessPolicy = new aws.iam.Policy(`pipeline-artifact-bucket-access`, {
      name: `pipeline-artifact-bucket-access`,
      policy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [{
          Action: ["s3:*"],
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

    new aws.iam.RolePolicyAttachment(`pipeline-codebuild-access`, {
      role: this.role,
      policyArn: aws.iam.ManagedPolicies.AWSCodeBuildDeveloperAccess
    });
    
    new aws.iam.RolePolicyAttachment(`pipeline-cloudwatch-access`, {
      role: this.role,
      policyArn: aws.iam.ManagedPolicies.CloudWatchFullAccessV2
    });

    const codeConnectionPolicy = new aws.iam.Policy(`pipeline-codeconnection-access-policy`, {
      name: `pipeline-codeconnection-access-policy`,
      policy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [{
          Action: [
            "codestar-connections:*",
				    "codeconnections:*"
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

    const codepipelinePolicy = new aws.iam.Policy(`pipeline-codepipeline-access-policy`, {
      name: `pipeline-codepipeline-access-policy`,
      policy: pulumi.jsonStringify({
        Version: "2012-10-17",
        Statement: [{
          Action: ["codepipeline:*"],
          Effect: "Allow",
          Resource: "*",
        }],
      }),
    });

    new aws.iam.RolePolicyAttachment(`pipeline-codepipeline-access`, {
      role: this.role,
      policyArn: codepipelinePolicy.arn
    });

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
