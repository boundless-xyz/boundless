import * as path from 'path';
import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createRustLambda } from './rust-lambda';

export interface RedriveLambdaArgs {
  /** Private subnets for Lambda to attach to */
  privSubNetIds: pulumi.Input<pulumi.Input<string>[]>;
  /** RDS writer URL secret (for DB writes) */
  dbUrlSecret: aws.secretsmanager.Secret;
  /** Indexer Security Group ID (that has access to RDS) */
  indexerSgId: pulumi.Input<string>;
  /** RUST_LOG level */
  rustLogLevel: string;
}

export class RedriveLambda extends pulumi.ComponentResource {
  public readonly lambdaFunction: aws.lambda.Function;

  constructor(
    name: string,
    args: RedriveLambdaArgs,
    opts?: pulumi.ComponentResourceOptions,
  ) {
    super(name, name, opts);

    const serviceName = `${name}-redrive`;

    const role = new aws.iam.Role(
      `${serviceName}-role`,
      {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({ Service: 'lambda.amazonaws.com' }),
      },
      { parent: this },
    );

    new aws.iam.RolePolicyAttachment(
      `${serviceName}-logs`,
      {
        role: role.name,
        policyArn: aws.iam.ManagedPolicies.AWSLambdaBasicExecutionRole,
      },
      { parent: this },
    );

    new aws.iam.RolePolicyAttachment(
      `${serviceName}-vpc-access`,
      {
        role: role.name,
        policyArn: aws.iam.ManagedPolicies.AWSLambdaVPCAccessExecutionRole,
      },
      { parent: this },
    );

    const inlinePolicy = pulumi.all([args.dbUrlSecret.arn]).apply(([secretArn]) =>
      JSON.stringify({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['secretsmanager:GetSecretValue'],
            Resource: [secretArn],
          },
        ],
      }),
    );

    new aws.iam.RolePolicy(
      `${serviceName}-policy`,
      {
        role: role.id,
        policy: inlinePolicy,
      },
      { parent: this },
    );

    const dbUrl = pulumi.all([args.dbUrlSecret]).apply(([secret]) => {
      const dbUrl = aws.secretsmanager.getSecretVersionOutput({
        secretId: secret.id,
      }).secretString;
      return dbUrl;
    });

    const { lambda } = createRustLambda(serviceName, {
      projectPath: path.join(__dirname, '../../../'),
      packageName: 'indexer-redrive',
      release: true,
      role: role.arn,
      environmentVariables: {
        DB_URL: dbUrl,
        RUST_LOG: args.rustLogLevel,
      },
      memorySize: 128,
      timeout: 30,
      vpcConfig: {
        subnetIds: args.privSubNetIds,
        securityGroupIds: [args.indexerSgId],
      },
    });
    this.lambdaFunction = lambda;

    this.registerOutputs({ lambdaFunction: this.lambdaFunction });
  }
}
