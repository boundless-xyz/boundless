import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export async function setupStorage(
    name: string,
    tags: Record<string, string>
) {
    // Create S3 bucket for workflow artifacts
    const workflowBucket = new aws.s3.Bucket(`${name}-workflow`, {
        bucketPrefix: `${name}-workflow-`,
        versioning: {
            enabled: true,
        },
        lifecycleRules: [{
            enabled: true,
            id: "cleanup-old-versions",
            noncurrentVersionExpiration: {
                days: 30,
            },
        }],
        serverSideEncryptionConfiguration: {
            rule: {
                applyServerSideEncryptionByDefault: {
                    sseAlgorithm: "AES256",
                },
            },
        },
        tags: {
            ...tags,
            Name: `${name}-workflow`,
        },
    });

    // Create S3 bucket for configurations
    const configBucket = new aws.s3.Bucket(`${name}-config`, {
        bucketPrefix: `${name}-config-`,
        acl: "private",
        versioning: {
            enabled: true,
        },
        serverSideEncryptionConfiguration: {
            rule: {
                applyServerSideEncryptionByDefault: {
                    sseAlgorithm: "AES256",
                },
            },
        },
        tags: {
            ...tags,
            Name: `${name}-config`,
        },
    });

    // Block public access to buckets
    const workflowBucketPab = new aws.s3.BucketPublicAccessBlock(`${name}-workflow-pab`, {
        bucket: workflowBucket.id,
        blockPublicAcls: true,
        blockPublicPolicy: true,
        ignorePublicAcls: true,
        restrictPublicBuckets: true,
    });

    const configBucketPab = new aws.s3.BucketPublicAccessBlock(`${name}-config-pab`, {
        bucket: configBucket.id,
        blockPublicAcls: true,
        blockPublicPolicy: true,
        ignorePublicAcls: true,
        restrictPublicBuckets: true,
    });

    // Create IAM user for S3 bucket access
    const agentUser = new aws.iam.User(`${name}-workflow-agent`, {
        name: `${name}-workflow-agent`,
        tags: {
            ...tags,
            Name: `${name}-workflow-agent`,
        },
    });

    // Create IAM policy for S3 bucket access
    const bucketPolicy = new aws.iam.UserPolicy(`${name}-bucket-access`, {
        user: agentUser.name,
        policy: pulumi.all([workflowBucket.arn, configBucket.arn]).apply(([workflowArn, configArn]) => 
            JSON.stringify({
                Version: "2012-10-17",
                Statement: [{
                    Action: ["s3:*"],
                    Effect: "Allow",
                    Resource: [
                        workflowArn, 
                        `${workflowArn}/*`,
                        configArn,
                        `${configArn}/*`
                    ],
                }],
            })
        ),
    });

    // Create access key for the IAM user
    const accessKey = new aws.iam.AccessKey(`${name}-agent-key`, {
        user: agentUser.name,
    });

    return {
        bucket: workflowBucket,
        configBucket: configBucket,
        bucketName: workflowBucket.id,
        configBucketName: configBucket.id,
        s3AccessKeyId: accessKey.id,
        s3SecretKey: accessKey.secret,
    };
}