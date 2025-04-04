import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export interface BasePipelineArgs {
  connection: aws.codestarconnections.Connection;
  artifactBucket: aws.s3.Bucket;
  role: aws.iam.Role;
  githubToken: pulumi.Output<string>;
  dockerUsername: string;
  dockerToken: pulumi.Output<string>;
  slackAlertsTopicArn: pulumi.Output<string>;
}