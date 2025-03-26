import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as awsx from "@pulumi/awsx";

const opsAccountPipelineRoleArn = "arn:aws:iam::968153779208:role/pipeline-role-3b97f1a";

const deploymentRole = new aws.iam.Role("deploymentRole", {
  assumeRolePolicy: pulumi.jsonStringify({
    Version: "2012-10-17",
    Statement: [
      {
        Action: "sts:AssumeRole",
        Principal: {
          AWS: opsAccountPipelineRoleArn
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
