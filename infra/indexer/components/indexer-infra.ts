import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';
import * as crypto from 'crypto';

export interface IndexerInfraArgs {
  serviceName: string;
  vpcId: pulumi.Output<string>;
  privSubNetIds: pulumi.Output<string[]>;
  rdsPassword: pulumi.Output<string>;
}

export class IndexerShared extends pulumi.ComponentResource {
  public readonly ecrRepository: awsx.ecr.Repository;
  public readonly ecrAuthToken: pulumi.Output<aws.ecr.GetAuthorizationTokenResult>;
  public readonly cacheBucket: aws.s3.BucketV2;
  public readonly indexerSecurityGroup: aws.ec2.SecurityGroup;
  public readonly rdsSecurityGroupId: pulumi.Output<string>;
  public readonly dbUrlSecret: aws.secretsmanager.Secret;
  public readonly dbUrlSecretVersion: aws.secretsmanager.SecretVersion;
  public readonly dbReaderUrlSecret: aws.secretsmanager.Secret;
  public readonly dbReaderUrlSecretVersion: aws.secretsmanager.SecretVersion;
  public readonly secretHash: pulumi.Output<string>;
  public readonly executionRole: aws.iam.Role;
  public readonly taskRole: aws.iam.Role;
  public readonly taskRolePolicyAttachment: aws.iam.RolePolicyAttachment;
  public readonly cluster: aws.ecs.Cluster;
  public readonly databaseVersion: string;

  constructor(name: string, args: IndexerInfraArgs, opts?: pulumi.ComponentResourceOptions) {
    super('indexer:infra', name, opts);

    const { vpcId, privSubNetIds, rdsPassword } = args;
    const serviceName = `${args.serviceName}-base`;

    this.ecrRepository = new awsx.ecr.Repository(`${serviceName}-repo`, {
      lifecyclePolicy: {
        rules: [
          {
            description: 'Delete untagged images after N days',
            tagStatus: 'untagged',
            maximumAgeLimit: 7,
          },
        ],
      },
      forceDelete: true,
      name: `${serviceName}-repo`,
    }, { parent: this });

    this.ecrAuthToken = aws.ecr.getAuthorizationTokenOutput({
      registryId: this.ecrRepository.repository.registryId,
    });

    this.cacheBucket = new aws.s3.BucketV2(`${serviceName}-cache`, {
      bucket: `${serviceName}-cache`,
    }, { parent: this });

    new aws.s3.BucketServerSideEncryptionConfigurationV2(`${serviceName}-cache-encryption`, {
      bucket: this.cacheBucket.id,
      rules: [
        {
          applyServerSideEncryptionByDefault: {
            sseAlgorithm: 'AES256',
          },
        },
      ],
    }, { parent: this });

    new aws.s3.BucketOwnershipControls(`${serviceName}-cache-ownership`, {
      bucket: this.cacheBucket.id,
      rule: {
        objectOwnership: 'BucketOwnerEnforced',
      },
    }, { parent: this });

    this.indexerSecurityGroup = new aws.ec2.SecurityGroup(`${serviceName}-sg`, {
      name: `${serviceName}-sg`,
      vpcId,
      egress: [
        {
          fromPort: 0,
          toPort: 0,
          protocol: '-1',
          cidrBlocks: ['0.0.0.0/0'],
          ipv6CidrBlocks: ['::/0'],
        },
      ],
    }, { parent: this });



    const dbSubnets = new aws.rds.SubnetGroup(`${serviceName}-dbsubnets`, {
      subnetIds: privSubNetIds,
    }, { parent: this });

    const rdsPort = 5432;
    const rdsSecurityGroup = new aws.ec2.SecurityGroup(`${serviceName}-rds`, {
      name: `${serviceName}-rds`,
      vpcId,
      ingress: [
        {
          fromPort: rdsPort,
          toPort: rdsPort,
          protocol: 'tcp',
          securityGroups: [this.indexerSecurityGroup.id],
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
    }, { parent: this });

    const rdsUser = 'indexer';

    // Note: changing this causes the database to be deleted, and then recreated from scratch, and indexing to restart.
    const databaseVersion = 'v19';
    this.databaseVersion = databaseVersion;
    const rdsDbName = `indexer${databaseVersion}`;

    const auroraCluster = new aws.rds.Cluster(`${serviceName}-aurora-${databaseVersion}`, {
      engine: 'aurora-postgresql',
      engineVersion: '17.4',
      clusterIdentifier: `${serviceName}-aurora-${databaseVersion}`,
      databaseName: rdsDbName,
      masterUsername: rdsUser,
      masterPassword: rdsPassword,
      port: rdsPort,
      backupRetentionPeriod: 7,
      skipFinalSnapshot: true,
      dbSubnetGroupName: dbSubnets.name,
      vpcSecurityGroupIds: [rdsSecurityGroup.id],
      storageEncrypted: true,
    }, { parent: this /* protect: true */ });

    new aws.rds.ClusterInstance(`${serviceName}-aurora-writer-${databaseVersion}`, {
      clusterIdentifier: auroraCluster.id,
      engine: 'aurora-postgresql',
      engineVersion: '17.4',
      instanceClass: 'db.r6g.large',
      identifier: `${serviceName}-aurora-writer-${databaseVersion}`,
      publiclyAccessible: false,
      dbSubnetGroupName: dbSubnets.name,
    }, { parent: this /* protect: true */ });

    new aws.rds.ClusterInstance(`${serviceName}-aurora-reader-${databaseVersion}`, {
      clusterIdentifier: auroraCluster.id,
      engine: 'aurora-postgresql',
      engineVersion: '17.4',
      instanceClass: 'db.r6g.large',
      identifier: `${serviceName}-aurora-reader-${databaseVersion}`,
      publiclyAccessible: false,
      dbSubnetGroupName: dbSubnets.name,
    }, { parent: this /* protect: true */ });

    // RDS Proxy for reader (Lambda API reads only)
    /*
    // Create IAM role for reader proxy
    const readerProxyRoleName = `${serviceName}-reader-proxy-role-${databaseVersion}`;
    const readerProxyRole = new aws.iam.Role(readerProxyRoleName, {
      name: readerProxyRoleName,
      assumeRolePolicy: JSON.stringify({
        Version: '2012-10-17',
        Statement: [{
          Effect: 'Allow',
          Principal: { Service: 'rds.amazonaws.com' },
          Action: 'sts:AssumeRole',
        }],
      }),
    }, { parent: this });

    // Create secret for reader proxy authentication FIRST (proxy needs to reference it)
    const readerProxySecret = new aws.secretsmanager.Secret(`${serviceName}-reader-proxy-secret-${databaseVersion}`, {
      name: `${serviceName}-reader-proxy-secret`,
    }, { parent: this });

    new aws.secretsmanager.SecretVersion(`${serviceName}-reader-proxy-secret-ver-${databaseVersion}`, {
      secretId: readerProxySecret.id,
      secretString: pulumi.interpolate`{"username":"${rdsUser}","password":"${rdsPassword}"}`,
    }, { parent: this });

    // Attach policy to reader proxy role for secret access
    new aws.iam.RolePolicy(`${serviceName}-reader-proxy-policy-${databaseVersion}`, {
      role: readerProxyRole.id,
      policy: readerProxySecret.arn.apply(arn => JSON.stringify({
        Version: '2012-10-17',
        Statement: [{
          Effect: 'Allow',
          Action: ['secretsmanager:GetSecretValue'],
          Resource: arn,
        }],
      })),
    }, { parent: this });

    // Create RDS Proxy (references secret created above)
    const readerProxy = new aws.rds.Proxy(`${serviceName}-reader-proxy-${databaseVersion}`, {
      name: `${serviceName}-reader-proxy-${databaseVersion}`,
      engineFamily: 'POSTGRESQL',
      auths: [{
        authScheme: 'SECRETS',
        iamAuth: 'DISABLED',
        secretArn: readerProxySecret.arn,  // Use actual secret ARN with proper suffix
      }],
      roleArn: readerProxyRole.arn,
      vpcSubnetIds: privSubNetIds,
      vpcSecurityGroupIds: [rdsSecurityGroup.id],
      requireTls: false,  // Secret-based auth doesn't use TLS between client and proxy (both in VPC)
      idleClientTimeout: 300, // 5 minutes for Lambda
    }, { parent: this });

    new aws.rds.ProxyDefaultTargetGroup(`${serviceName}-reader-proxy-tg-${databaseVersion}`, {
      dbProxyName: readerProxy.name,
      connectionPoolConfig: {
        maxConnectionsPercent: 90,
        maxIdleConnectionsPercent: 50,
        connectionBorrowTimeout: 60,
      },
    }, { parent: this });

    new aws.rds.ProxyTarget(`${serviceName}-reader-proxy-target-${databaseVersion}`, {
      dbProxyName: readerProxy.name,
      targetGroupName: 'default',
      dbClusterIdentifier: auroraCluster.clusterIdentifier,
    }, { parent: this });
    */

    // Writer secret: direct connection to Aurora cluster endpoint (for ECS indexer)
    const dbUrlSecretValue = pulumi.interpolate`postgres://${rdsUser}:${rdsPassword}@${auroraCluster.endpoint}:${rdsPort}/${rdsDbName}?sslmode=require`;
    this.dbUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-db-url-${databaseVersion}`, {}, { parent: this });
    this.dbUrlSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-db-url-ver-${databaseVersion}`, {
      secretId: this.dbUrlSecret.id,
      secretString: dbUrlSecretValue,
    }, { parent: this, dependsOn: [this.dbUrlSecret] });

    // Reader secret: direct connection to Aurora reader endpoint (for Lambda API)
    // Note: Using Aurora cluster reader endpoint which automatically routes to reader instances
    const dbReaderUrlSecretValue = pulumi.interpolate`postgres://${rdsUser}:${rdsPassword}@${auroraCluster.readerEndpoint}:${rdsPort}/${rdsDbName}?sslmode=require`;
    this.dbReaderUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-db-reader-url-${databaseVersion}`, {}, { parent: this });
    this.dbReaderUrlSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-db-reader-url-ver-${databaseVersion}`, {
      secretId: this.dbReaderUrlSecret.id,
      secretString: dbReaderUrlSecretValue,
    }, { parent: this, dependsOn: [this.dbReaderUrlSecret] });

    this.secretHash = pulumi
      .all([dbUrlSecretValue, this.dbUrlSecretVersion.arn, dbReaderUrlSecretValue, this.dbReaderUrlSecretVersion.arn])
      .apply(([writerValue, writerVersionArn, readerValue, readerVersionArn]) => {
        const hash = crypto.createHash('sha1');
        hash.update(writerValue);
        hash.update(writerVersionArn);
        hash.update(readerValue);
        hash.update(readerVersionArn);
        return hash.digest('hex');
      });

    const dbSecretAccessPolicy = new aws.iam.Policy(`${serviceName}-db-url-policy`, {
      policy: pulumi.all([this.dbUrlSecret.arn, this.dbReaderUrlSecret.arn]).apply(([writerArn, readerArn]): aws.iam.PolicyDocument => ({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
            Resource: [writerArn, readerArn],
          },
        ],
      })),
    }, { parent: this, dependsOn: [this.dbUrlSecret, this.dbReaderUrlSecret] });

    this.executionRole = new aws.iam.Role(`${serviceName}-ecs-role`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
        Service: 'ecs-tasks.amazonaws.com',
      }),
    }, { parent: this });

    this.ecrRepository.repository.arn.apply((repoArn) => {
      new aws.iam.RolePolicy(`${serviceName}-ecs-pol`, {
        role: this.executionRole.id,
        policy: {
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              // GetAuthorizationToken is an account-level AWS ECR action
              // and does not support resource-level permissions. Must use '*'.
              // See: https://docs.aws.amazon.com/AmazonECR/latest/userguide/security-iam-awsmanpol.html
              Action: ['ecr:GetAuthorizationToken'],
              Resource: '*',
            },
            {
              Effect: 'Allow',
              Action: [
                'ecr:BatchCheckLayerAvailability',
                'ecr:GetDownloadUrlForLayer',
                'ecr:BatchGetImage',
              ],
              Resource: repoArn,
            },
            {
              Effect: 'Allow',
              Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
              Resource: [this.dbUrlSecret.arn, this.dbReaderUrlSecret.arn],
            },
          ],
        },
      }, { parent: this });
    });

    this.cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, {
      name: `${serviceName}-cluster`,
    }, { parent: this, dependsOn: [this.executionRole, this.dbUrlSecretVersion] });

    this.taskRole = new aws.iam.Role(`${serviceName}-task`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
        Service: 'ecs-tasks.amazonaws.com',
      }),
      managedPolicyArns: [aws.iam.ManagedPolicy.AmazonECSTaskExecutionRolePolicy],
    }, { parent: this });

    this.taskRolePolicyAttachment = new aws.iam.RolePolicyAttachment(`${serviceName}-task-policy`, {
      role: this.taskRole.id,
      policyArn: dbSecretAccessPolicy.arn,
    }, { parent: this });

    new aws.iam.RolePolicy(`${serviceName}-task-s3-policy`, {
      role: this.taskRole.id,
      policy: this.cacheBucket.arn.apply((bucketArn) => JSON.stringify({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['s3:*'],
            Resource: `${bucketArn}/*`,
          },
          {
            Effect: 'Allow',
            Action: ['s3:*'],
            Resource: bucketArn,
          },
        ],
      })),
    }, { parent: this });

    new aws.iam.RolePolicy(`${serviceName}-execution-s3-policy`, {
      role: this.executionRole.id,
      policy: this.cacheBucket.arn.apply((bucketArn) => JSON.stringify({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['s3:*'],
            Resource: `${bucketArn}/*`,
          },
          {
            Effect: 'Allow',
            Action: ['s3:*'],
            Resource: bucketArn,
          },
        ],
      })),
    }, { parent: this });

    new aws.s3.BucketPolicy(`${serviceName}-cache-policy`, {
      bucket: this.cacheBucket.id,
      policy: pulumi.all([this.cacheBucket.arn, this.taskRole.arn, this.executionRole.arn]).apply(([bucketArn, taskRoleArn, executionRoleArn]) =>
        JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Principal: {
                AWS: [taskRoleArn, executionRoleArn],
              },
              Action: ['s3:*'],
              Resource: `${bucketArn}/*`,
            },
            {
              Effect: 'Allow',
              Principal: {
                AWS: [taskRoleArn, executionRoleArn],
              },
              Action: ['s3:*'],
              Resource: bucketArn,
            },
          ],
        })
      ),
    }, { parent: this, dependsOn: [this.taskRole, this.executionRole] });

    this.rdsSecurityGroupId = rdsSecurityGroup.id;

    /**
    // CloudWatch Alarms for Database Connections and Replication
    // Note: r6g.xlarge has a max of 2000 connections according to AWS docs
    const maxConnections = 2000;
    const connectionThreshold = maxConnections * 0.8; // 80% threshold

    // Alarm for Aurora Writer connections
    new aws.cloudwatch.MetricAlarm(`${serviceName}-aurora-writer-connections-${databaseVersion}`, {
      name: `${serviceName}-aurora-writer-connections-${databaseVersion}`,
      comparisonOperator: 'GreaterThanThreshold',
      evaluationPeriods: 2,
      metricName: 'DatabaseConnections',
      namespace: 'AWS/RDS',
      period: 300, // 5 minutes
      statistic: 'Average',
      threshold: connectionThreshold,
      alarmDescription: `Aurora writer instance connections exceeding 80% (${connectionThreshold} connections)`,
      dimensions: {
        DBClusterIdentifier: auroraCluster.clusterIdentifier,
        Role: 'WRITER',
      },
      treatMissingData: 'notBreaching',
    }, { parent: this });

    // Alarm for Aurora Reader connections
    new aws.cloudwatch.MetricAlarm(`${serviceName}-aurora-reader-connections-${databaseVersion}`, {
      name: `${serviceName}-aurora-reader-connections-${databaseVersion}`,
      comparisonOperator: 'GreaterThanThreshold',
      evaluationPeriods: 2,
      metricName: 'DatabaseConnections',
      namespace: 'AWS/RDS',
      period: 300, // 5 minutes
      statistic: 'Average',
      threshold: connectionThreshold,
      alarmDescription: `Aurora reader instance connections exceeding 80% (${connectionThreshold} connections)`,
      dimensions: {
        DBClusterIdentifier: auroraCluster.clusterIdentifier,
        Role: 'READER',
      },
      treatMissingData: 'notBreaching',
    }, { parent: this });

    // Alarm for Aurora Replication Lag
    new aws.cloudwatch.MetricAlarm(`${serviceName}-aurora-replication-lag-${databaseVersion}`, {
      name: `${serviceName}-aurora-replication-lag-${databaseVersion}`,
      comparisonOperator: 'GreaterThanThreshold',
      evaluationPeriods: 2,
      metricName: 'AuroraReplicaLag',
      namespace: 'AWS/RDS',
      period: 60, // 1 minute
      statistic: 'Average',
      threshold: 500, // 500ms
      alarmDescription: 'Aurora replication lag exceeding 500ms',
      dimensions: {
        DBClusterIdentifier: auroraCluster.clusterIdentifier,
      },
      treatMissingData: 'notBreaching',
    }, { parent: this });

    // Alarm for RDS Reader Proxy Client Connections (Lambda API)
    new aws.cloudwatch.MetricAlarm(`${serviceName}-reader-proxy-connections-${databaseVersion}`, {
      name: `${serviceName}-reader-proxy-connections-${databaseVersion}`,
      comparisonOperator: 'GreaterThanThreshold',
      evaluationPeriods: 2,
      metricName: 'ClientConnections',
      namespace: 'AWS/RDS',
      period: 300, // 5 minutes
      statistic: 'Average',
      threshold: 90, // Alert if reader proxy has > 90 client connections (25 lambda limit × 3 connections + buffer)
      alarmDescription: 'RDS Reader Proxy client connections approaching Lambda concurrency limit',
      dimensions: {
        ProxyName: readerProxy.name,
      },
      treatMissingData: 'notBreaching',
    }, { parent: this });
    **/

    this.registerOutputs({
      repositoryUrl: this.ecrRepository.repository.repositoryUrl,
      dbUrlSecretArn: this.dbUrlSecret.arn,
      dbReaderUrlSecretArn: this.dbReaderUrlSecret.arn,
      rdsSecurityGroupId: this.rdsSecurityGroupId,
      taskRoleArn: this.taskRole.arn,
      executionRoleArn: this.executionRole.arn,
    });
  }
}
