import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as pulumi from '@pulumi/pulumi';

import { getServiceNameV1 } from '../../util';
import * as crypto from 'crypto';

const FARGATE_CPU = 512;
const FARGATE_MEMORY = 512;

interface LoadTestTriggerArgs {
  chainId: string;
  stackName: string;
  privateKey: pulumi.Output<string>;
  pinataJWT: pulumi.Output<string>;
  ethRpcUrl: pulumi.Output<string>;
  indexerUrl: pulumi.Output<string>;
  orderStreamUrl: pulumi.Output<string>;
  imageRef: pulumi.Input<string>;
  logLevel: string;
  setVerifierAddr: string;
  boundlessMarketAddr: string;
  ipfsGateway: string;
  lockCollateralRaw: string;
  vpcId: pulumi.Output<string>;
  privateSubnetIds: pulumi.Output<string[]>;
}

export class LoadTestTrigger extends pulumi.ComponentResource {
  public readonly lambdaName: pulumi.Output<string>;

  constructor(args: LoadTestTriggerArgs, opts?: pulumi.ComponentResourceOptions) {
    const name = 'load-test';
    super(`boundless:order-generator:${name}`, name, args, opts);

    const serviceName = getServiceNameV1(args.stackName, `og-${name}`, args.chainId);

    // Secrets
    const privateKeySecret = new aws.secretsmanager.Secret(`${serviceName}-private-key`, {}, { parent: this });
    new aws.secretsmanager.SecretVersion(`${serviceName}-private-key-v1`, {
      secretId: privateKeySecret.id,
      secretString: args.privateKey,
    }, { parent: this });

    const pinataJwtSecret = new aws.secretsmanager.Secret(`${serviceName}-pinata-jwt`, {}, { parent: this });
    new aws.secretsmanager.SecretVersion(`${serviceName}-pinata-jwt-v1`, {
      secretId: pinataJwtSecret.id,
      secretString: args.pinataJWT,
    }, { parent: this });

    const rpcUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-rpc-url`, {}, { parent: this });
    new aws.secretsmanager.SecretVersion(`${serviceName}-rpc-url`, {
      secretId: rpcUrlSecret.id,
      secretString: args.ethRpcUrl,
    }, { parent: this });

    const orderStreamUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-order-stream-url`, {}, { parent: this });
    new aws.secretsmanager.SecretVersion(`${serviceName}-order-stream-url`, {
      secretId: orderStreamUrlSecret.id,
      secretString: args.orderStreamUrl,
    }, { parent: this });

    const indexerUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-indexer-url`, {}, { parent: this });
    new aws.secretsmanager.SecretVersion(`${serviceName}-indexer-url`, {
      secretId: indexerUrlSecret.id,
      secretString: args.indexerUrl,
    }, { parent: this });

    const secretHash = pulumi
      .all([args.ethRpcUrl, args.privateKey, args.orderStreamUrl])
      .apply(([rpcUrl, pk, osUrl]: [string, string, string]) => {
        const hash = crypto.createHash('sha1');
        hash.update(rpcUrl);
        hash.update(pk);
        hash.update(osUrl ?? '');
        return hash.digest('hex');
      });

    // Networking
    const securityGroup = new aws.ec2.SecurityGroup(`${serviceName}-security-group`, {
      name: serviceName,
      vpcId: args.vpcId,
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

    // ECS execution role
    const execRole = new aws.iam.Role(`${serviceName}-exec`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
        Service: 'ecs-tasks.amazonaws.com',
      }),
      managedPolicyArns: [aws.iam.ManagedPolicy.AmazonECSTaskExecutionRolePolicy],
    }, { parent: this });

    const execRolePolicy = new aws.iam.RolePolicy(`${serviceName}-exec`, {
      role: execRole.id,
      policy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
            Resource: [
              privateKeySecret.arn,
              pinataJwtSecret.arn,
              rpcUrlSecret.arn,
              orderStreamUrlSecret.arn,
              indexerUrlSecret.arn,
            ],
          },
        ],
      },
    }, { parent: this });

    // Log group
    const logGroupName = `${serviceName}-logs`;
    const logGroup = new aws.cloudwatch.LogGroup(logGroupName, {
      name: logGroupName,
      retentionInDays: 30,
    }, { parent: this });

    // ECS cluster
    const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, {
      name: serviceName,
    }, { parent: this });

    // Task definition
    const containerName = `${serviceName}-container`;
    const taskDef = new awsx.ecs.FargateTaskDefinition(`${serviceName}-task`, {
      container: {
        name: containerName,
        image: args.imageRef,
        cpu: FARGATE_CPU,
        memory: FARGATE_MEMORY,
        essential: true,
        environment: [
          { name: 'IPFS_GATEWAY_URL', value: args.ipfsGateway },
          { name: 'RUST_LOG', value: args.logLevel },
          { name: 'NO_COLOR', value: '1' },
          { name: 'SECRET_HASH', value: secretHash },
        ],
        secrets: [
          { name: 'RPC_URL', valueFrom: rpcUrlSecret.arn },
          { name: 'PRIVATE_KEY', valueFrom: privateKeySecret.arn },
          { name: 'PINATA_JWT', valueFrom: pinataJwtSecret.arn },
          { name: 'INDEXER_URL', valueFrom: indexerUrlSecret.arn },
          { name: 'ORDER_STREAM_URL', valueFrom: orderStreamUrlSecret.arn },
        ],
      },
      logGroup: { existing: logGroup },
      executionRole: { roleArn: execRole.arn },
    }, { parent: this, dependsOn: [execRole, execRolePolicy] });

    // Lambda IAM role
    const lambdaRole = new aws.iam.Role(`${serviceName}-lambda-role`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({ Service: 'lambda.amazonaws.com' }),
      managedPolicyArns: [aws.iam.ManagedPolicy.AWSLambdaBasicExecutionRole],
    }, { parent: this });

    new aws.iam.RolePolicy(`${serviceName}-lambda-policy`, {
      role: lambdaRole.id,
      policy: pulumi.all([taskDef.taskDefinition.arn, execRole.arn, taskDef.taskDefinition.taskRoleArn, cluster.arn]).apply(
        ([taskDefArn, execRoleArn, taskRoleArn, clusterArn]) => JSON.stringify({
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Action: ['ecs:RunTask'],
              Resource: taskDefArn.replace(/:\d+$/, ':*'),
            },
            {
              Effect: 'Allow',
              Action: ['ecs:StopTask', 'ecs:DescribeTasks'],
              Resource: '*',
              Condition: {
                ArnEquals: { 'ecs:cluster': clusterArn },
              },
            },
            {
              Effect: 'Allow',
              Action: ['iam:PassRole'],
              Resource: [execRoleArn, taskRoleArn],
            },
          ],
        })
      ),
    }, { parent: this });

    // Lambda function
    const lambda = new aws.lambda.Function(`${serviceName}-trigger`, {
      name: `${serviceName}-trigger`,
      role: lambdaRole.arn,
      runtime: 'nodejs20.x',
      handler: 'index.handler',
      timeout: 30,
      code: new pulumi.asset.AssetArchive({
        '.': new pulumi.asset.FileArchive('../order-generator/load-test-trigger-lambda/build'),
      }),
      environment: {
        variables: {
          CLUSTER_ARN: cluster.arn,
          TASK_DEFINITION_ARN: taskDef.taskDefinition.arn,
          SUBNET_IDS: args.privateSubnetIds.apply(ids => ids.join(',')),
          SECURITY_GROUP_ID: securityGroup.id,
          CONTAINER_NAME: containerName,
          BOUNDLESS_ADDRESS: args.boundlessMarketAddr,
          SET_VERIFIER_ADDRESS: args.setVerifierAddr,
        },
      },
    }, { parent: this, dependsOn: [lambdaRole] });

    this.lambdaName = lambda.name;
  }
}
