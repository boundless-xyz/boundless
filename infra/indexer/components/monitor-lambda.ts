import * as path from 'path';
import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createRustLambda } from './rust-lambda';
import { ChainId, getChainId, Stage } from '../../util';
import { buildCreateMetricFns } from './alarms';
import { alarmConfig } from '../alarmConfig';

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
  /** Boundless alerts topic ARNs */
  boundlessAlertsTopicArns?: string[];
  /** Namespace for service metrics, e.g. operation health of the monitor/indexer infra */
  serviceMetricsNamespace: string;
  /** Namespace for market metrics, e.g. order volume, order count, etc. */
  marketMetricsNamespace: string;
}

export class MonitorLambda extends pulumi.ComponentResource {
  public readonly lambdaFunction: aws.lambda.Function;

  constructor(
    name: string,
    args: MonitorLambdaArgs,
    opts?: pulumi.ComponentResourceOptions,
  ) {
    super(name, name, opts);

    const serviceName = name;
    const chainId: ChainId = getChainId(args.chainId);
    const stage = pulumi.getStack().includes("staging") ? Stage.STAGING : Stage.PROD;
    const chainStageAlarmConfig = alarmConfig[chainId][stage];

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

    const { marketMetricsNamespace, serviceMetricsNamespace } = args;

    const { lambda, logGroupName } = createRustLambda(`${serviceName}-monitor`, {
      projectPath: path.join(__dirname, '../../../'),
      packageName: 'indexer-monitor',
      release: true,
      role: role.arn,
      environmentVariables: {
        DB_URL: dbUrl,
        RUST_LOG: args.rustLogLevel,
        CLOUDWATCH_NAMESPACE: marketMetricsNamespace,
      },
      memorySize: 128,
      timeout: 30,
      vpcConfig: {
        subnetIds: args.privSubNetIds,
        securityGroupIds: [lambdaSg.id],
      },
    },
    );
    this.lambdaFunction = lambda;

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-monitor-error-filter`, {
      name: `${serviceName}-monitor-log-err-filter`,
      logGroupName: logGroupName,
      metricTransformation: {
        namespace: serviceMetricsNamespace,
        name: `${serviceName}-monitor-log-err`,
        value: '1',
        defaultValue: '0',
      },
      pattern: '?ERROR ?error ?Error',
    }, { dependsOn: [this.lambdaFunction] });

    const alarmActions = args.boundlessAlertsTopicArns ?? [];

    // 2 errors within 1 hour in the order generator triggers a SEV2 alarm.
    new aws.cloudwatch.MetricAlarm(`${serviceName}-monitor-error-alarm`, {
      name: `${serviceName}-monitor-log-err`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: serviceMetricsNamespace,
            metricName: `${serviceName}-monitor-log-err`,
            period: 60,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 1,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 60,
      datapointsToAlarm: 2,
      treatMissingData: 'notBreaching',
      alarmDescription: `Monitor Lambda ${name} log ERROR level`,
      actionsEnabled: true,
      alarmActions,
    });


    const rule = new aws.cloudwatch.EventRule(
      `${name}-schedule`,
      {
        scheduleExpression: `rate(${args.intervalMinutes} minute)`
      },
      { parent: this },
    );

    if (chainStageAlarmConfig) {
      const clientAddresses = chainStageAlarmConfig.clients.map(c => c.address);
      const proverAddresses = chainStageAlarmConfig.provers.map(p => p.address);

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
      const { createMetricAlarm, createSuccessRateAlarm } = buildCreateMetricFns(serviceName, marketMetricsNamespace, alarmActions);
      const { fulfilledRequests, submittedRequests, expiredRequests, slashedRequests } = chainStageAlarmConfig.topLevel;

      fulfilledRequests.forEach(({ severity, description, metricConfig, alarmConfig }) => {
        createMetricAlarm({
          metricName: "fulfilled_requests_number",
          severity,
          description,
          metricConfig,
          alarmConfig,
        });
      });

      submittedRequests.forEach(({ severity, description, metricConfig, alarmConfig }) => {
        createMetricAlarm({
          metricName: "requests_number",
          severity,
          description,
          metricConfig,
          alarmConfig,
        });
      });

      expiredRequests.forEach(({ severity, description, metricConfig, alarmConfig }) => {
        createMetricAlarm({
          metricName: "expired_requests_number",
          severity,
          description,
          metricConfig,
          alarmConfig,
        });
      });

      slashedRequests.forEach(({ severity, description, metricConfig, alarmConfig }) => {
        createMetricAlarm({
          metricName: "slashed_requests_number",
          severity,
          description,
          metricConfig,
          alarmConfig,
        });
      });

      const { clients: clientAlarms } = chainStageAlarmConfig;
      // Create alarms for each client
      clientAlarms.forEach((client) => {
        const { submissionRate, successRate, name, address } = client;
        if (submissionRate != null) {
          submissionRate.forEach((cfg) => {
            const { severity, description, metricConfig, alarmConfig } = cfg;
            createMetricAlarm({
              metricName: "requests_number_from",
              severity,
              target: { name, address },
              description,
              metricConfig,
              alarmConfig,
            });
          });
        };

        if (successRate != null) {
          successRate.forEach((cfg) => {
            const { severity, description, metricConfig, alarmConfig } = cfg;
            createSuccessRateAlarm({ name, address }, severity, description, metricConfig, alarmConfig);
          });
        };

      });
    }

    this.registerOutputs({ lambdaFunction: this.lambdaFunction });
  }
}
