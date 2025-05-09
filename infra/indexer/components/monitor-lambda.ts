import * as path from 'path';
import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createRustLambda } from './rust-lambda';
import { getServiceNameV1 } from '../../util';

export interface MonitorLambdaArgs {
  /** VPC where RDS lives */
  vpcId: pulumi.Input<string>;
  /** Private subnets for Lambda to attach to */
  privSubNetIds: pulumi.Input<pulumi.Input<string>[]>;
  /** How often (in minutes) to run */
  intervalMinutes: string;
  /** RDS Url secret */
  dbUrlSecret: aws.secretsmanager.Secret;
  /** Chain ID */
  chainId: string;
  rustLogLevel: string;
  /** Lambda input payload (optional) */
  lambdaInput?: {
    clients: string[];
    provers: string[];
  };
}

const SERVICE_NAME_BASE = 'indexer';

export class MonitorLambda extends pulumi.ComponentResource {
  public readonly lambdaFunction: aws.lambda.Function;

  constructor(
    name: string,
    args: MonitorLambdaArgs,
    opts?: pulumi.ComponentResourceOptions,
  ) {
    super(`${SERVICE_NAME_BASE}-${args.chainId}`, name, opts);

    const stackName = pulumi.getStack();
    const serviceName = getServiceNameV1(stackName, SERVICE_NAME_BASE);

    const role = new aws.iam.Role(
      `${name}-role`,
      {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({ Service: 'lambda.amazonaws.com' }),
      },
      { parent: this },
    );

    new aws.iam.RolePolicyAttachment(
      `${name}-logs`,
      {
        role: role.name,
        policyArn: aws.iam.ManagedPolicies.AWSLambdaBasicExecutionRole,
      },
      { parent: this },
    );

    new aws.iam.RolePolicyAttachment(
      `${name}-vpc-access`,
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
          {
            Effect: 'Allow',
            Action: ['cloudwatch:PutMetricData'],
            Resource: ['*'],
          },
        ],
      }),
    );

    new aws.iam.RolePolicy(
      `${name}-policy`,
      {
        role: role.id,
        policy: inlinePolicy,
      },
      { parent: this },
    );

    const lambdaSg = new aws.ec2.SecurityGroup(
      `${name}-sg`,
      {
        vpcId: args.vpcId,
        description: 'Lambda SG for DB access',
        egress: [
          {
            protocol: '-1',
            fromPort: 0,
            toPort: 0,
            cidrBlocks: ['0.0.0.0/0'],
          },
        ],
      },
      { parent: this },
    );

    const rdsSg = aws.ec2.getSecurityGroupOutput({
      filters: [
        { name: 'group-name', values: [`${serviceName}-rds`] },
        { name: 'vpc-id', values: [args.vpcId] },
      ],
    });

    new aws.ec2.SecurityGroupRule(
      `${name}-sg-ingress-rds`,
      {
        type: 'ingress',
        fromPort: 5432,
        toPort: 5432,
        protocol: 'tcp',
        securityGroupId: rdsSg.id,
        sourceSecurityGroupId: lambdaSg.id,
      },
      { parent: this },
    );

    const dbUrl = aws.secretsmanager.getSecretVersionOutput({
      secretId: args.dbUrlSecret.id,
    }).secretString;

    const intervalSeconds = parseInt(args.intervalMinutes) * 60;
    this.lambdaFunction = createRustLambda(`${serviceName}-monitor`, {
      projectPath: path.join(__dirname, '../../../'),
      packageName: 'indexer-monitor',
      release: true,
      role: role.arn,
      environmentVariables: {
        DB_URL: dbUrl,
        RUST_LOG: 'info',
        INTERVAL_SECONDS: intervalSeconds.toString(),
        CLOUDWATCH_NAMESPACE: `${serviceName}-monitor-${args.chainId}`,
      },
      memorySize: 128,
      timeout: 30,
      vpcConfig: {
        subnetIds: args.privSubNetIds,
        securityGroupIds: [lambdaSg.id],
      },
    },
    );

    const rule = new aws.cloudwatch.EventRule(
      `${name}-schedule`,
      {
        scheduleExpression: `rate(${args.intervalMinutes} minute)`
      },
      { parent: this },
    );

    // Default payload if not provided
    const defaultLambdaInput = {
      clients: [],
      provers: []
    };

    new aws.cloudwatch.EventTarget(
      `${name}-target`,
      { rule: rule.name, arn: this.lambdaFunction.arn, input: JSON.stringify(args.lambdaInput ?? defaultLambdaInput) },
      { parent: this },
    );
    new aws.lambda.Permission(
      `${name}-perm`,
      {
        action: 'lambda:InvokeFunction',
        function: this.lambdaFunction.name,
        principal: 'events.amazonaws.com',
        sourceArn: rule.arn,
      },
      { parent: this },
    );

    this.registerOutputs({ lambdaFunction: this.lambdaFunction });
  }
}
