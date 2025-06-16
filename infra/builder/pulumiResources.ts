import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

const BOUNDLESS_DEV_ADMIN_ROLE_ARN = "arn:aws:iam::751442549745:role/aws-reserved/sso.amazonaws.com/us-east-2/AWSReservedSSO_AWSAdministratorAccess_05b42ccedab0fe1d";

// Pulumi state bucket and secret key just used for the builder.
// Builder is just deployed to dev, so does not use the state bucket that we use for staging/prod.
export const createPulumiState = (): {
  bucket: aws.s3.BucketV2,
  keyAlias: aws.kms.Alias,
} => {
  const bucket = new aws.s3.BucketV2(
      'boundless-builder-state-bucket',
      {
      bucketPrefix: 'boundless-builder-state',
    },
    {
      protect: true,
      retainOnDelete: true,
    }
  );

  const bucketPolicy: aws.iam.PolicyDocument = {
    Version: "2012-10-17",
    Statement: [
        {
            "Effect": "Allow",
            "Principal": {
                "AWS": [
                  BOUNDLESS_DEV_ADMIN_ROLE_ARN,
                ]
            },
            "Action": [
                "s3:GetObject",
                "s3:ListBucket",
                "s3:PutObject",
                "s3:DeleteObject",
            ],
            "Resource": [
                pulumi.interpolate`${bucket.arn}`,
                pulumi.interpolate`${bucket.arn}/*`
            ]
        }
    ]
  };

  new aws.s3.BucketPolicy("builder-state-bucket-policy", {
    bucket: bucket.id,
    policy: pulumi.jsonStringify(bucketPolicy),
  });

  const pulumiSecretsKey = new aws.kms.Key(
    'pulumiSecretsKey',
    {
      description: 'KMS Key for Pulumi secrets',
      deletionWindowInDays: 7,
      enableKeyRotation: true,
    },
    {
      protect: true,
    }
  );

  const keyAlias = new aws.kms.Alias('builder-secrets-key-alias', {
    name: 'alias/builder-secrets-key',
    targetKeyId: pulumiSecretsKey.keyId,
  });

  const keyPolicyDoc: aws.iam.PolicyDocument = {
    Id: 'Boundless builder secrets encryption bucket key policy',
    Version: '2012-10-17',
    Statement: [
      {
        Sid: 'Allow admins full access to the key',
        Effect: 'Allow',
        Principal: {
          AWS: BOUNDLESS_DEV_ADMIN_ROLE_ARN,
        },
        Action: 'kms:*',
        Resource: '*',
      }
    ],
  };

  new aws.kms.KeyPolicy('builder-secrets-key-policy', {
    keyId: pulumiSecretsKey.id,
    policy: pulumi.jsonStringify(keyPolicyDoc),
  });

  return { bucket, keyAlias };
}