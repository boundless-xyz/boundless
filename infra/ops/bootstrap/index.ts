import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

const OPS_ACCOUNT_PIPELINE_ROLE_ARN = "arn:aws:iam::968153779208:role/pipeline-role-3b97f1a";

// Create a deployment role that can be used to deploy to the current account.
const deploymentRole = new aws.iam.Role("deploymentRole", {
  assumeRolePolicy: pulumi.jsonStringify({
    Version: "2012-10-17",
    Statement: [
      {
        Action: "sts:AssumeRole",
        Principal: {
          AWS: OPS_ACCOUNT_PIPELINE_ROLE_ARN
        },
        Effect: "Allow",
        Sid: ""
      }
    ]
  }),
  managedPolicyArns: [
    aws.iam.ManagedPolicies.AdministratorAccess
  ]
});

// Export the name of the bucket
export const deploymentRoleArn = deploymentRole.arn;
