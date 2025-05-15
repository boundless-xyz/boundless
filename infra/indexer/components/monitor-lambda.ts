import * as path from 'path';
import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createRustLambda } from './rust-lambda';
import { getServiceNameV1, Severity } from '../../util';
import { clients, clientAddresses, proverAddresses } from './targets';
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
  /** RDS sg ID */
  rdsSgId: pulumi.Input<string>;
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
          {
            Effect: 'Allow',
            Action: ['cloudwatch:PutMetricData'],
            Resource: ['*'],
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

    const lambdaSg = new aws.ec2.SecurityGroup(
      `${serviceName}-sg`,
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

    new aws.ec2.SecurityGroupRule(
      `${serviceName}-sg-ingress-rds`,
      {
        type: 'ingress',
        fromPort: 5432,
        toPort: 5432,
        protocol: 'tcp',
        securityGroupId: args.rdsSgId,
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
      clients: clientAddresses,
      provers: proverAddresses,
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

    createMetricAlarm("fulfilled_requests_number", Severity.SEV1,
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

    createMetricAlarm("requests_number", Severity.SEV2,
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

    createMetricAlarm("expired_requests_number", Severity.SEV2,
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

    createMetricAlarm("slashed_requests_number", Severity.SEV2,
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
        client.submissionRate.forEach((cfg) => {
          const severity = cfg.severity;
          const metricConfig = cfg.metricConfig;
          const alarmConfig = cfg.alarmConfig;

          createMetricAlarm(`requests_number_from`, severity,
            client,
            `${alarmConfig.datapointsToAlarm} missed submissions in ${metricConfig.period} seconds`, metricConfig,
            {
              threshold: 1,
              comparisonOperator: "LessThanThreshold",
              ...alarmConfig,
            },
          );
        });
      };

      if (client.successRate != null) {
        client.successRate.forEach((cfg) => {
          const severity = cfg.severity;
          const metricConfig = cfg.metricConfig;
          const alarmConfig = cfg.alarmConfig;

          createSuccessRateAlarm(client, severity, `success rate is below ${alarmConfig.threshold}`, metricConfig,
            alarmConfig
          );
        });
      };

    });

    this.registerOutputs({ lambdaFunction: this.lambdaFunction });
  }
}
