import * as aws from "@pulumi/aws";
import * as pulumi from "pulumi";

const isDev = pulumi.getStack() === "dev";

const config = new pulumi.Config();

const sampleSecret = isDev ? process.env.SAMPLE_SECRET : config.requireSecret("sample:sampleSecret");

// Create an AWS resource (S3 Bucket)
const bucket = new aws.s3.BucketV2("my-sample-boundless-bucket");

// Export the name of the bucket
export const bucketName = bucket.id;
