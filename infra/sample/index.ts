import * as aws from "@pulumi/aws";

// Create an AWS resource (S3 Bucket)
const bucket = new aws.s3.BucketV2("my-sample-boundless-bucket");
const bucket2 = new aws.s3.BucketV2("my-sample-boundless-bucket-2");

// Export the name of the bucket
export const bucketName = bucket.id;
export const bucketName2 = bucket2.id;
