import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as vpc from './lib/vpc';
import { getEnvVar } from '../util';

const OPS_ACCOUNT_PIPELINE_ROLE_ARN = "arn:aws:iam::968153779208:role/pipeline-role-3b97f1a";

const stackName = pulumi.getStack();
const isDev = stackName.includes("dev");
const prefix = isDev ? getEnvVar("DEV_NAME") : "";

export = async () => {
  // Create a deployment role that can be used to deploy to the current account.
  const deploymentRole = new aws.iam.Role(`${prefix}deploymentRole`, {
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

  let availabilityZones = (await aws.getAvailabilityZones()).names;
  
  // For dev, we only use one AZ to limit the number of EIPs that are created.
  if (isDev) {
    availabilityZones = [availabilityZones[0]];
  }

  const awsRegion = (await aws.getRegion({})).name;
  const services_vpc = new vpc.Vpc(`${prefix}vpc`, {
    region: awsRegion,
    availabilityZones,
  });

  return {
    DEPLOYMENT_ROLE_ARN: deploymentRole.arn, 
    VPC_ID: services_vpc.vpcx.vpcId,
    PRIVATE_SUBNET_IDS: services_vpc.vpcx.privateSubnetIds,
    PUBLIC_SUBNET_IDS: services_vpc.vpcx.publicSubnetIds,
  }
}
