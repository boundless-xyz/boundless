import * as path from 'path';
import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createRustLambda } from './rust-lambda';
import { getServiceNameV1, Severity } from '../../util';
import { clients, provers } from './targets';
import { createMetricAlarm, createSuccessRateAlarm } from './alarms';

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
  /** RUST_LOG level */
  rustLogLevel: string;
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

    this.lambdaFunction = createRustLambda(`${serviceName}-monitor`, {
      projectPath: path.join(__dirname, '../../../'),
      packageName: 'indexer-monitor',
      release: true,
      role: role.arn,
      environmentVariables: {
        DB_URL: dbUrl,
        RUST_LOG: 'info',
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

    const payload = {
      clients: clients,
      provers: provers,
    };

    new aws.cloudwatch.EventTarget(
      `${name}-target`,
      { rule: rule.name, arn: this.lambdaFunction.arn, input: JSON.stringify(payload) },
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

    // Create top level alarms
    // Total Number of Fulfilled Orders - SEV1: <5 within 10 minutes

    createMetricAlarm("requests_fulfilled", Severity.SEV1,
      undefined,
      "less than 5 fulfilled orders in 10 minutes", {
      period: 600
    },
      {
        threshold: 5,
        comparisonOperator: "LessThanThreshold",
      },
    );

    // Total Number of Submitted Orders - SEV2: TBD(needs baseline)

    createMetricAlarm("requests_submitted", Severity.SEV2,
      undefined,
      "less than 5 submitted orders in 10 minutes", {
      period: 600
    },
      {
        threshold: 5,
        comparisonOperator: "LessThanThreshold",
      },
    );


    // Total Number of Expired Orders - SEV2: TBD(needs baseline)

    createMetricAlarm("requests_expired", Severity.SEV2,
      undefined,
      "at least 2 expired orders in 10 minutes", {
      period: 600
    },
      {
        threshold: 2,
        comparisonOperator: "GreaterThanOrEqualToThreshold",
      },
    );

    // Total Number of Slashed Orders - SEV2: TBD(needs baseline)

    createMetricAlarm("requests_slashed", Severity.SEV2,
      undefined,
      "at least 2 slashed orders in 10 minutes", {
      period: 600
    },
      {
        threshold: 2,
        comparisonOperator: "GreaterThanOrEqualToThreshold",
      },
    );

    // Create alarms for each client
    clients.forEach((client) => {
      if (client.submissionRate != null) {
        const period = client.submissionRate;
        createMetricAlarm("requests_submitted", Severity.SEV1,
          client,
          "two missed submissions in an hour", {
          period: period
        },
          {
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            evaluationPeriods: 12,
            datapointsToAlarm: 2,
          },
        );
        createMetricAlarm("requests_submitted", Severity.SEV2,
          client,
          "one missed submissions in an hour", {
          period: period
        },
          {
            threshold: 1,
            comparisonOperator: "LessThanThreshold",
            evaluationPeriods: 12,
            datapointsToAlarm: 2,
          },
        );
      };

      if (client.successRate != null) {
        const rate = client.successRate;
        createSuccessRateAlarm(client, Severity.SEV2, `success rate is below $rate`,
          { period: 3600 },
          { threshold: rate }
        );

      }

    });

    this.registerOutputs({ lambdaFunction: this.lambdaFunction });
  }
}
