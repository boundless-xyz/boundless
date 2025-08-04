import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export type Storage = {
    bucket: aws.s3.BucketV2;
    configBucket: aws.s3.BucketV2;
    bucketName: pulumi.Output<string>;
    configBucketName: pulumi.Output<string>;
    s3AccessKeyId: pulumi.Output<string>;
    s3SecretKey: pulumi.Output<string>;
};

export async function setupStorage(
    name: string,
    tags: Record<string, string>
): Promise<Storage> {
    // Create S3 bucket for workflow artifacts
    const workflowBucket = new aws.s3.BucketV2(`${name}-workflow`, {
        bucketPrefix: `${name}-workflow-`,
        tags: {
            ...tags,
            Name: `${name}-workflow`,
        },
    });

    const workflowBucketOwnershipControls = new aws.s3.BucketOwnershipControls(`${name}-workflow-ownership`, {
        bucket: workflowBucket.id,
        rule: {
            objectOwnership: "BucketOwnerPreferred",
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

    const workflowBucketAcl = new aws.s3.BucketAclV2(`${name}-workflow-acl`, {
        bucket: workflowBucket.id,
        acl: "private",
    }, {
        dependsOn: [workflowBucketOwnershipControls, workflowBucketPab],
    });

    const workflowLifecycleRule = new aws.s3.BucketLifecycleConfigurationV2(`${name}-workflow-lifecycle`, {
        bucket: workflowBucket.id,
        rules: [{
            id: "cleanup-old-versions",
            status: "Enabled",
            noncurrentVersionExpiration: {
                noncurrentDays: 30,
            },
        }],
    });

    const workflowBucketVersioning = new aws.s3.BucketVersioningV2(`${name}-workflow-versioning`, {
        bucket: workflowBucket.id,
        versioningConfiguration: {
            status: "Enabled",
        },
    });
    const workflowBucketSSEConfig = new aws.s3.BucketServerSideEncryptionConfigurationV2(`${name}-workflow-sse`, {
        bucket: workflowBucket.id,
        rules: [{
            applyServerSideEncryptionByDefault: {
                sseAlgorithm: "AES256",
            },
        }],
    });

    const workflowBucketCors = new aws.s3.BucketCorsConfigurationV2(`${name}-workflow-cors`, {
        bucket: workflowBucket.id,
        corsRules: [{
            allowedHeaders: ["*"],
            allowedMethods: ["GET", "PUT", "POST", "DELETE"],
            allowedOrigins: ["*"],
            exposeHeaders: ["ETag"],
            maxAgeSeconds: 3000,
        }],
    });

    // Create S3 bucket for configurations
    const configBucket = new aws.s3.BucketV2(`${name}-config`, {
        bucketPrefix: `${name}-config-`,
        tags: {
            ...tags,
            Name: `${name}-config`,
        },
    });

    const configBucketOwnershipControls = new aws.s3.BucketOwnershipControls(`${name}-config-ownership`, {
        bucket: configBucket.id,
        rule: {
            objectOwnership: "BucketOwnerPreferred",
        },
    });

    const configBucketPab = new aws.s3.BucketPublicAccessBlock(`${name}-config-pab`, {
        bucket: configBucket.id,
        blockPublicAcls: true,
        blockPublicPolicy: true,
        ignorePublicAcls: true,
        restrictPublicBuckets: true,
    });

    const configBucketAcl = new aws.s3.BucketAclV2(`${name}-config-acl`, {
        bucket: configBucket.id,
        acl: "private",
    }, {
        dependsOn: [configBucketOwnershipControls, configBucketPab],
    });

    const configBucketVersioning = new aws.s3.BucketVersioningV2(`${name}-config-versioning`, {
        bucket: configBucket.id,
        versioningConfiguration: {
            status: "Enabled",
        },
    });
    const configBucketSSEConfig = new aws.s3.BucketServerSideEncryptionConfigurationV2(`${name}-config-sse`, {
        bucket: configBucket.id,
        rules: [{
            applyServerSideEncryptionByDefault: {
                sseAlgorithm: "AES256",
            },
        }],
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

    // TODO: Consider creating a user here rather than just granting permissions to the roles that need access to this bucket
    // Create access key for the IAM user
    const accessKey = new aws.iam.AccessKey(`${name}-agent-key`, {
        user: agentUser.name,
    });

    return {
        bucket: workflowBucket,
        configBucket,
        bucketName: workflowBucket.id,
        configBucketName: configBucket.id,
        s3AccessKeyId: accessKey.id,
        s3SecretKey: accessKey.secret,
    };
}