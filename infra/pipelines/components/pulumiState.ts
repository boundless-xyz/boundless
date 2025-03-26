import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export class PulumiStateBucket extends pulumi.ComponentResource {
  public bucket: aws.s3.BucketV2;
  public kmsKey: aws.kms.Alias;

  constructor(
    name: string,
    args: {
      accountId: string;
      readWriteStateBucketArns: string[];
    },
    opts?: pulumi.ComponentResourceOptions
  ) {
    super('pipelines:PulumiState', name, args, opts);

    // Create key used for encrypting the Pulumi state bucket
    const pulumiStateBucketKey = new aws.kms.Key(
      'pulumiStateBucketStacksKey',
      {
        description: 'KMS Key for Pulumi state bucket',
        deletionWindowInDays: 7,
        enableKeyRotation: true,
      },
      {
        parent: this,
        protect: true,
      }
    );

    this.kmsKey = new aws.kms.Alias(
      'pulumiStateBucketKeyAlias',
      {
        name: 'alias/pulumi-stacks-key',
        targetKeyId: pulumiStateBucketKey.keyId,
      },
      {
        parent: this,
      }
    );

    // Key policy for the Pulumi state bucket key. Accessing the bucket also requires
    // access to the key.
    const keyPolicy: aws.iam.PolicyDocument = {
      Id: 'Boundless Pulumi State Bucket Key Policy',
      Version: '2012-10-17',
      Statement: [
        {
          Sid: 'Enable IAM User Permissions',
          Effect: 'Allow',
          Principal: {
            AWS: `arn:aws:iam::${args.accountId}:root`,
          },
          Action: 'kms:*',
          Resource: '*',
        },
        {
          Principal: {
            AWS: args.readWriteStateBucketArns,
          },
          Effect: 'Allow',
          Action: ['kms:Encrypt', 'kms:Decrypt', 'kms:ReEncrypt*', 'kms:GenerateDataKey*', 'kms:DescribeKey'],
          Resource: '*',
          Sid: 'Allow principals to use KMS key',
        },
      ],
    };

    new aws.kms.KeyPolicy(
      'pulumiStateBucketKeyPolicy',
      {
        keyId: pulumiStateBucketKey.id,
        policy: pulumi.jsonStringify(keyPolicy),
      },
      {
        parent: this,
      }
    );

    // Boundless Pulumi backend state bucket. Used by all pipelines and staging/prod 
    // for tracking state for their deployments. Not used for local development.
    this.bucket = new aws.s3.BucketV2(
      'boundlessPulumiStateBucket',
      {
        bucket: 'boundless-pulumi-state',
      },
      {
        parent: this,
        protect: true,
      }
    );

    new aws.s3.BucketServerSideEncryptionConfigurationV2(
      "pulumiStateBucketSSEConfiguration", {
      bucket: this.bucket.id,
      rules: [{
          applyServerSideEncryptionByDefault: {
              kmsMasterKeyId: this.kmsKey.arn,
              sseAlgorithm: "aws:kms",
          },
      }],
    });

    new aws.s3.BucketVersioningV2(
      'pulumiStateBucketVersioning',
      {
        bucket: this.bucket.id,
        versioningConfiguration: {
          status: 'Enabled',
        },
      },
      {
        parent: this,
      }
    );

    new aws.s3.BucketLifecycleConfigurationV2(
      'pulumiStateBucketLifecycle',
      {
        bucket: this.bucket.id,
        rules: [
          {
            id: 'rule',
            status: 'Enabled',
            filter: {},
            noncurrentVersionExpiration: {
              noncurrentDays: 30,
              newerNoncurrentVersions: 3,
            },
          },
        ],
      },
      {
        parent: this,
      }
    );

    // Grants read/write access to the Pulumi state bucket to the given principals
    const bucketPolicy: aws.iam.PolicyDocument = {
      Version: "2012-10-17",
      Statement: [
          {
              "Effect": "Allow",
              "Principal": {
                  "AWS": args.readWriteStateBucketArns
              },
              "Action": [
                  "s3:GetObject",
                  "s3:ListBucket",
                  "s3:PutObject",
                  "s3:DeleteObject"
              ],
              "Resource": [
                  pulumi.interpolate`${this.bucket.arn}`,
                  pulumi.interpolate`${this.bucket.arn}/*`
              ]
          }
      ]
    };

    new aws.s3.BucketPolicy("pulumiStateBucketPolicy", {
      bucket: this.bucket.id,
      policy: pulumi.jsonStringify(bucketPolicy),
    });

  }
}
