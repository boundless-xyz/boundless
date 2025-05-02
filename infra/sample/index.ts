import * as aws from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import { getEnvVar, getServiceNameV1 } from "../util";

const isDev = pulumi.getStack() === "dev";
const serviceName = getServiceNameV1(pulumi.getStack(), "sample");
const config = new pulumi.Config();

const baseStack = new pulumi.StackReference("organization/bootstrap/services-dev");
const baseStackVpcId = baseStack.getOutput('VPC_ID');
const baseStackPrivSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS');

const sampleSecret = isDev ? getEnvVar("SAMPLE_SECRET") : config.requireSecret("sampleSecret");

// Create an AWS resource (S3 Bucket)
const bucket = new aws.s3.BucketV2(`${serviceName}-bucket`);

// Export the name of the bucket
export const bucketName = bucket.id;
export const vpcId = baseStackVpcId;
export const privSubNetIds = baseStackPrivSubNetIds;
