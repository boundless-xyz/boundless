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
  public readonly indexerSecurityGroup: aws.ec2.SecurityGroup;
  public readonly rdsSecurityGroupId: pulumi.Output<string>;
  public readonly dbUrlSecret: aws.secretsmanager.Secret;
  public readonly dbUrlSecretVersion: aws.secretsmanager.SecretVersion;
  public readonly secretHash: pulumi.Output<string>;
  public readonly executionRole: aws.iam.Role;
  public readonly taskRole: aws.iam.Role;
  public readonly taskRolePolicyAttachment: aws.iam.RolePolicyAttachment;
  public readonly cluster: aws.ecs.Cluster;

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

    const rdsUser = 'indexer';
    const rdsPort = 5432;
    const rdsDbName = 'indexerV1';

    const dbSubnets = new aws.rds.SubnetGroup(`${serviceName}-dbsubnets`, {
      subnetIds: privSubNetIds,
    }, { parent: this });

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

    const auroraCluster = new aws.rds.Cluster(`${serviceName}-aurora-v1`, {
      engine: 'aurora-postgresql',
      engineVersion: '17.4',
      clusterIdentifier: `${serviceName}-aurora-v1`,
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

    new aws.rds.ClusterInstance(`${serviceName}-aurora-writer-1`, {
      clusterIdentifier: auroraCluster.id,
      engine: 'aurora-postgresql',
      engineVersion: '17.4',
      instanceClass: 'db.t4g.medium',
      identifier: `${serviceName}-aurora-writer-v1`,
      publiclyAccessible: false,
      dbSubnetGroupName: dbSubnets.name,
    }, { parent: this /* protect: true */ });

    const dbUrlSecretValue = pulumi.interpolate`postgres://${rdsUser}:${rdsPassword}@${auroraCluster.endpoint}:${rdsPort}/${rdsDbName}?sslmode=require`;
    this.dbUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-db-url`, {}, { parent: this });
    this.dbUrlSecretVersion = new aws.secretsmanager.SecretVersion(`${serviceName}-db-url-ver`, {
      secretId: this.dbUrlSecret.id,
      secretString: dbUrlSecretValue,
    }, { parent: this });

    this.secretHash = pulumi
      .all([dbUrlSecretValue, this.dbUrlSecretVersion.arn])
      .apply(([value, versionArn]) => {
        const hash = crypto.createHash('sha1');
        hash.update(value);
        hash.update(versionArn);
        return hash.digest('hex');
      });

    const dbSecretAccessPolicy = new aws.iam.Policy(`${serviceName}-db-url-policy`, {
      policy: this.dbUrlSecret.arn.apply((secretArn): aws.iam.PolicyDocument => ({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
            Resource: [secretArn],
          },
        ],
      })),
    }, { parent: this });

    this.executionRole = new aws.iam.Role(`${serviceName}-ecs-execution-role`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
        Service: 'ecs-tasks.amazonaws.com',
      }),
    }, { parent: this });

    this.ecrRepository.repository.arn.apply((repoArn) => {
      new aws.iam.RolePolicy(`${serviceName}-ecs-execution-pol`, {
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
              Resource: [this.dbUrlSecret.arn],
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

    this.rdsSecurityGroupId = rdsSecurityGroup.id;

    this.registerOutputs({
      repositoryUrl: this.ecrRepository.repository.repositoryUrl,
      dbUrlSecretArn: this.dbUrlSecret.arn,
      rdsSecurityGroupId: this.rdsSecurityGroupId,
      taskRoleArn: this.taskRole.arn,
      executionRoleArn: this.executionRole.arn,
    });
  }
}
