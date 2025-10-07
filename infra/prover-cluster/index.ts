import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { getServiceNameV1 } from "../util";

const serviceName = getServiceNameV1(pulumi.getStack(), "prover-cluster");
const bucket = new aws.s3.BucketV2(`${serviceName}-bucket`);
export const bucketName = bucket.id;
