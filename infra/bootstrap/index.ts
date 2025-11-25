import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as vpc from './lib/vpc';
import { getEnvVar } from '../util';

const OPS_ACCOUNT_PIPELINE_ROLE_ARN = "arn:aws:iam::968153779208:role/pipeline-role-3b97f1a";
const OPS_ACCOUNT_LOG_LAMBDA_ROLE_ARN = "arn:aws:iam::968153779208:role/chatbot-log-fetcher-role-4d93cb1"

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

  const awsRegion = (await aws.getRegion({})).name;
  const services_vpc = new vpc.Vpc(`${prefix}vpc`, {
    region: awsRegion,
    availabilityZones,
  });

  // Configure some ECR properties
  const basicScanTypeVersion = new aws.ecr.AccountSetting("ecrBasicScanTypeVersion", {
    name: "BASIC_SCAN_TYPE_VERSION",
    value: "AWS_NATIVE",
  });
  const configuration = new aws.ecr.RegistryScanningConfiguration("ecrRegistryScanningConfiguration", {
    scanType: "BASIC",
    rules: [{
      scanFrequency: "SCAN_ON_PUSH",
      repositoryFilters: [{
        filter: "*",
        filterType: "WILDCARD",
      }],
    }],
  });

  // Create a role that the log fetcher Lambda can assume to list tags on Cloudwatch alarms in the current account
  const alarmTagsRole = new aws.iam.Role(`${prefix}alarmListTagsRole`, {
    name: `${prefix}alarmListTagsRole`,
    assumeRolePolicy: pulumi.jsonStringify({
      Version: "2012-10-17",
      Statement: [
        {
          Action: "sts:AssumeRole",
          Principal: {
            AWS: OPS_ACCOUNT_LOG_LAMBDA_ROLE_ARN
          },
          Effect: "Allow",
          Sid: ""
        }
      ]
    }),
    inlinePolicies: [
      {
        name: 'cloudwatchAlarmListTagsPolicy',
        policy: pulumi.jsonStringify({
          Version: "2012-10-17",
          Statement: [{
            Sid: "AllowListTagsAnyAlarm",
            Effect: "Allow",
            Action: "cloudwatch:ListTagsForResource",
            Resource: pulumi.interpolate`arn:aws:cloudwatch:us-west-2:${aws.getCallerIdentityOutput().accountId}:alarm:*`
          }],
        }),
      }
    ]
  });

  return {
    DEPLOYMENT_ROLE_ARN: deploymentRole.arn,
    VPC_ID: services_vpc.vpcx.vpcId,
    PRIVATE_SUBNET_IDS: services_vpc.vpcx.privateSubnetIds,
    PUBLIC_SUBNET_IDS: services_vpc.vpcx.publicSubnetIds,
  }
}
