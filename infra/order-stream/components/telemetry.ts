import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export interface TelemetryInfraArgs {
  serviceName: string;
  vpcId: pulumi.Output<string>;
  pubSubNetIds: pulumi.Output<string[]>;
  redshiftAdminPassword: pulumi.Output<string>;
}

// Kinesis Data Streams, Redshift Serverless, and IAM wiring for broker telemetry.
// Redshift consumes from Kinesis via streaming ingestion (materialized views).
// The DDL for the external schema + MVs + views is in redshift-migrations/.
export class TelemetryInfra extends pulumi.ComponentResource {
  public heartbeatStreamName: pulumi.Output<string>;
  public evaluationsStreamName: pulumi.Output<string>;
  public completionsStreamName: pulumi.Output<string>;
  public heartbeatStreamArn: pulumi.Output<string>;
  public evaluationsStreamArn: pulumi.Output<string>;
  public completionsStreamArn: pulumi.Output<string>;
  public redshiftEndpoint: pulumi.Output<string>;
  public redshiftPort: pulumi.Output<number>;
  public redshiftIamRoleArn: pulumi.Output<string>;

  constructor(
    name: string,
    args: TelemetryInfraArgs,
    opts?: pulumi.ComponentResourceOptions
  ) {
    super('boundless:telemetry:TelemetryInfra', name, opts);

    const { serviceName, vpcId, pubSubNetIds, redshiftAdminPassword } = args;
    const retentionHours = 72;
    const version = 'v3';

    // Kinesis Data Streams

    const heartbeatStream = new aws.kinesis.Stream(
      `${serviceName}-telem-heartbeats`,
      {
        name: `${serviceName}-telem-heartbeats`,
        shardCount: 1,
        retentionPeriod: retentionHours,
        streamModeDetails: { streamMode: 'PROVISIONED' },
      },
      { parent: this }
    );

    const evaluationsStream = new aws.kinesis.Stream(
      `${serviceName}-telem-evaluations`,
      {
        name: `${serviceName}-telem-evaluations`,
        shardCount: 2,
        retentionPeriod: retentionHours,
        streamModeDetails: { streamMode: 'PROVISIONED' },
      },
      { parent: this }
    );

    const completionsStream = new aws.kinesis.Stream(
      `${serviceName}-telem-completions`,
      {
        name: `${serviceName}-telem-completions`,
        shardCount: 1,
        retentionPeriod: retentionHours,
        streamModeDetails: { streamMode: 'PROVISIONED' },
      },
      { parent: this }
    );

    // IAM role for Redshift to read from Kinesis (streaming ingestion)

    const redshiftKinesisRole = new aws.iam.Role(
      `${serviceName}-rs-kinesis-role`,
      {
        assumeRolePolicy: JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Principal: { Service: 'redshift.amazonaws.com' },
              Action: 'sts:AssumeRole',
            },
          ],
        }),
      },
      { parent: this }
    );

    new aws.iam.RolePolicy(
      `${serviceName}-rs-kinesis-policy`,
      {
        role: redshiftKinesisRole.id,
        policy: pulumi
          .all([
            heartbeatStream.arn,
            evaluationsStream.arn,
            completionsStream.arn,
          ])
          .apply(([hbArn, evalArn, compArn]) =>
            JSON.stringify({
              Version: '2012-10-17',
              Statement: [
                {
                  Sid: 'ReadStreams',
                  Effect: 'Allow',
                  Action: [
                    'kinesis:DescribeStreamSummary',
                    'kinesis:GetShardIterator',
                    'kinesis:GetRecords',
                    'kinesis:ListShards',
                    'kinesis:DescribeStream',
                  ],
                  Resource: [hbArn, evalArn, compArn],
                },
                {
                  Sid: 'ListStreams',
                  Effect: 'Allow',
                  Action: 'kinesis:ListStreams',
                  Resource: '*',
                },
              ],
            })
          ),
      },
      { parent: this }
    );

    // Redshift Serverless

    const redshiftSg = new aws.ec2.SecurityGroup(
      `${serviceName}-rs-sg`,
      {
        name: `${serviceName}-rs-sg`,
        vpcId,
        ingress: [
          {
            fromPort: 5439,
            toPort: 5439,
            protocol: 'tcp',
            cidrBlocks: ['0.0.0.0/0'],
            description: 'Public Redshift access',
          },
        ],
        egress: [
          {
            fromPort: 0,
            toPort: 0,
            protocol: '-1',
            cidrBlocks: ['0.0.0.0/0'],
          },
        ],
      },
      { parent: this }
    );

    const namespace = new aws.redshiftserverless.Namespace(
      `${serviceName}-rs-ns-${version}`,
      {
        namespaceName: `${serviceName}-telem-${version}`,
        adminUsername: 'admin',
        adminUserPassword: redshiftAdminPassword,
        dbName: 'telemetry',
        iamRoles: [redshiftKinesisRole.arn],
        defaultIamRoleArn: redshiftKinesisRole.arn,
      },
      { parent: this, deleteBeforeReplace: true }
    );

    const workgroup = new aws.redshiftserverless.Workgroup(
      `${serviceName}-rs-wg-${version}`,
      {
        workgroupName: `${serviceName}-telem-${version}`,
        namespaceName: namespace.namespaceName,
        baseCapacity: 8,
        publiclyAccessible: true,
        securityGroupIds: [redshiftSg.id],
        subnetIds: pubSubNetIds,
      },
      { parent: this, deleteBeforeReplace: true }
    );

    // Outputs

    this.heartbeatStreamName = heartbeatStream.name;
    this.evaluationsStreamName = evaluationsStream.name;
    this.completionsStreamName = completionsStream.name;
    this.heartbeatStreamArn = heartbeatStream.arn;
    this.evaluationsStreamArn = evaluationsStream.arn;
    this.completionsStreamArn = completionsStream.arn;
    this.redshiftEndpoint = workgroup.endpoints.apply(
      (eps) => eps?.[0]?.address ?? ''
    );
    this.redshiftPort = workgroup.endpoints.apply(
      (eps) => eps?.[0]?.port ?? 5439
    );
    this.redshiftIamRoleArn = redshiftKinesisRole.arn;

    this.registerOutputs({
      heartbeatStreamName: this.heartbeatStreamName,
      evaluationsStreamName: this.evaluationsStreamName,
      completionsStreamName: this.completionsStreamName,
      heartbeatStreamArn: this.heartbeatStreamArn,
      evaluationsStreamArn: this.evaluationsStreamArn,
      completionsStreamArn: this.completionsStreamArn,
      redshiftEndpoint: this.redshiftEndpoint,
      redshiftPort: this.redshiftPort,
      redshiftIamRoleArn: this.redshiftIamRoleArn,
    });
  }
}
