import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export class PulumiSecrets extends pulumi.ComponentResource {
  public kmsKey: aws.kms.Alias;

  constructor(
    name: string,
    args: {
      accountId: string;
      encryptKmsKeyArns: string[];
      decryptKmsKeyArns: string[];
    },
    opts?: pulumi.ComponentResourceOptions
  ) {
    super('pipelines:PulumiState', name, args, opts);

    const pulumiStacksKey = new aws.kms.Key(
      'pulumiSecretsKey',
      {
        description: 'KMS Key for Pulumi secrets',
        deletionWindowInDays: 7,
        enableKeyRotation: true,
      },
      {
        parent: this,
        protect: true,
      }
    );

    this.kmsKey = new aws.kms.Alias(
      'pulumiSecretsKeyAlias',
      {
        name: 'alias/pulumi-secrets-key',
        targetKeyId: pulumiStacksKey.keyId,
      },
      {
        parent: this,
      }
    );

    const keyPolicyDoc: aws.iam.PolicyDocument = {
      Id: 'Boundless Secrets Encryption Bucket Key Policy',
      Version: '2012-10-17',
      Statement: [
        {
          Sid: 'Allow admins of the ops account access to the key',
          Effect: 'Allow',
          Principal: {
            AWS: `arn:aws:iam::${args.accountId}:root`,
          },
          Action: 'kms:*',
          Resource: '*',
        },
        {
          Principal: {
            AWS: args.encryptKmsKeyArns,
          },
          Effect: 'Allow',
          Action: ['kms:Encrypt', 'kms:Decrypt', 'kms:ReEncrypt*', 'kms:GenerateDataKey*', 'kms:DescribeKey'],
          Resource: '*',
          Sid: 'Allow principals to encrypt and decrypt using KMS key',
        },
        {
          Principal: {
            AWS: args.decryptKmsKeyArns,
          },
          Effect: 'Allow',
          Action: ['kms:Decrypt', 'kms:GenerateDataKey*', 'kms:DescribeKey'],
          Resource: '*',
          Sid: 'Allow principals to decrypt using KMS key',
        },
      ],
    };

    new aws.kms.KeyPolicy(
      'pulumiSecretsKeyPolicy',
      {
        keyId: pulumiStacksKey.id,
        policy: pulumi.jsonStringify(keyPolicyDoc),
      },
      {
        parent: this,
      }
    );
  }
}
