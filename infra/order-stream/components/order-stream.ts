import * as fs from 'fs';
import * as aws from '@pulumi/aws';
import * as awsx from '@pulumi/awsx';
import * as docker_build from '@pulumi/docker-build';
import * as pulumi from '@pulumi/pulumi';
import { getServiceNameV1 } from '../../util';

const SERVICE_NAME_BASE = 'order-stream';
const HEALTH_CHECK_PATH = '/api/v1/health';

export class OrderStreamInstance extends pulumi.ComponentResource {
  public lbUrl: pulumi.Output<string>;
  public swaggerUrl: pulumi.Output<string>;

  constructor(
    name: string,
    args: {
      chainId: string;
      ciCacheSecret?: pulumi.Output<string>;
      dockerDir: string;
      dockerTag: string;
      orderStreamPingTime: number;
      privSubNetIds: pulumi.Output<string[]>;
      pubSubNetIds: pulumi.Output<string[]>;
      githubTokenSecret?: pulumi.Output<string>;
      minBalance: string;
      boundlessAddress: string;
      vpcId: pulumi.Output<string>;
      rdsPassword: pulumi.Output<string>;
      albDomain?: pulumi.Output<string>;
      ethRpcUrl: pulumi.Output<string>;
      bypassAddrs: string;
      boundlessAlertsTopicArn?: string;
    },
    opts?: pulumi.ComponentResourceOptions
  ) {
    super(`${SERVICE_NAME_BASE}-${args.chainId}`, name, opts);

    const {
      chainId,
      ciCacheSecret,
      dockerDir,
      dockerTag,
      orderStreamPingTime,
      privSubNetIds,
      pubSubNetIds,
      githubTokenSecret,
      minBalance,
      boundlessAddress,
      vpcId,
      rdsPassword,
      albDomain,
      ethRpcUrl,
      bypassAddrs,
    } = args;

    const stackName = pulumi.getStack();
    const serviceName = getServiceNameV1(stackName, SERVICE_NAME_BASE);

    // If we're in prod and have a domain, create a cert
    let cert: aws.acm.Certificate | undefined;
    if (stackName.includes('prod') && albDomain) {
      cert = new aws.acm.Certificate(`${serviceName}-cert`, {
        domainName: pulumi.interpolate`${albDomain}`,
        validationMethod: "DNS",
      }, { protect: true });
    }

    const ecrRepository = new awsx.ecr.Repository(`${serviceName}-repo`, {
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
    });

    const authToken = aws.ecr.getAuthorizationTokenOutput({
      registryId: ecrRepository.repository.registryId,
    });

    // Optionally add in the gh token secret and sccache s3 creds to the build ctx
    let buildSecrets = {};
    if (ciCacheSecret !== undefined) {
      const cacheFileData = ciCacheSecret.apply((filePath: any) => fs.readFileSync(filePath, 'utf8'));
      buildSecrets = {
        ci_cache_creds: cacheFileData,
      };
    }
    if (githubTokenSecret !== undefined) {
      buildSecrets = {
        ...buildSecrets,
        githubTokenSecret
      }
    }

    const image = new docker_build.Image(`${serviceName}-img`, {
      tags: [pulumi.interpolate`${ecrRepository.repository.repositoryUrl}:${dockerTag}`],
      context: {
        location: dockerDir,
      },
      platforms: ['linux/amd64'],
      push: true,
      dockerfile: {
        location: `${dockerDir}/dockerfiles/order_stream.dockerfile`,
      },
      buildArgs: {
        S3_CACHE_PREFIX: 'private/boundless/rust-cache-docker-Linux-X64/sccache',
      },
      secrets: buildSecrets,
      cacheFrom: [
        {
          registry: {
            ref: pulumi.interpolate`${ecrRepository.repository.repositoryUrl}:cache`,
          },
        },
      ],
      cacheTo: [
        {
          registry: {
            mode: docker_build.CacheMode.Max,
            imageManifest: true,
            ociMediaTypes: true,
            ref: pulumi.interpolate`${ecrRepository.repository.repositoryUrl}:cache`,
          },
        },
      ],
      registries: [
        {
          address: ecrRepository.repository.repositoryUrl,
          password: authToken.apply((authToken) => authToken.password),
          username: authToken.apply((authToken) => authToken.userName),
        },
      ],
    });

    // If we have a cert and a domain, use it, and enable https.
    // Otherwise we use the default lb endpoint that AWS provides that only supports http.
    let listener: awsx.types.input.lb.ListenerArgs;
    if (cert && albDomain) {
      listener = {
        port: 443,
        protocol: 'HTTPS',
        certificateArn: cert.arn.apply((arn) => arn),
      };
    } else {
      listener = {
        port: 80,
        protocol: 'HTTP',
      };
    }

    // Protect the load balancer so it doesn't get deleted if the stack is accidentally modified/deleted
    // Important as the A record of this resource is tied to DNS.
    const loadbalancer = new awsx.lb.ApplicationLoadBalancer(`${serviceName}-lb`, {
      name: `${serviceName}-lb`,
      subnetIds: pubSubNetIds,
      listener,
      defaultTargetGroup: {
        name: `${serviceName}-tg`,
        port: 8585,
        protocol: 'HTTP',
        deregistrationDelay: 10,
        healthCheck: {
          enabled: true,
          interval: 30,
          healthyThreshold: 2,
          port: '8585',
          protocol: 'HTTP',
          path: HEALTH_CHECK_PATH,
        },
      },
      // This should be slightly greater than the order-steam configured ping/pong time
      idleTimeout: orderStreamPingTime + orderStreamPingTime * 0.2,
    }, { protect: true });

    const orderStreamSecGroup = new aws.ec2.SecurityGroup(`${serviceName}-sg`, {
      name: `${serviceName}-sg`,
      vpcId: vpcId,
      ingress: [
        {
          fromPort: 0,
          toPort: 8585,
          protocol: 'tcp',
          securityGroups: [
            loadbalancer.defaultSecurityGroup.apply((secGroup) => {
              if (secGroup === undefined) {
                throw Error('ALB default security group is not defined');
              }
              return secGroup.id;
            }),
          ],
        },
      ],
      egress: [
        {
          fromPort: 0,
          toPort: 0,
          protocol: '-1',
          cidrBlocks: ['0.0.0.0/0'],
          ipv6CidrBlocks: ['::/0'],
        },
      ],
    });

    const rdsUser = 'worker';
    const rdsPort = 5432;
    const rdsDbName = 'orderstream';

    const dbSubnets = new aws.rds.SubnetGroup(`${serviceName}-dbsubnets`, {
      subnetIds: privSubNetIds,
    });

    const rdsSecurityGroup = new aws.ec2.SecurityGroup(`${serviceName}-rds`, {
      name: `${serviceName}-rds`,
      vpcId: vpcId,
      ingress: [
        {
          fromPort: rdsPort,
          toPort: rdsPort,
          protocol: 'tcp',
          securityGroups: [orderStreamSecGroup.id],
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
    });

    const rds = new aws.rds.Instance(`${serviceName}-rds`, {
      engine: 'postgres',
      engineVersion: '17.2',
      identifier: `${serviceName}-rds`,
      dbName: rdsDbName,
      username: rdsUser,
      password: rdsPassword,
      maxAllocatedStorage: 500,
      port: rdsPort,
      instanceClass: 'db.t4g.small',
      allocatedStorage: 20,
      storageEncrypted: true,
      skipFinalSnapshot: true,
      publiclyAccessible: false,
      dbSubnetGroupName: dbSubnets.name,
      vpcSecurityGroupIds: [rdsSecurityGroup.id],
      storageType: 'gp3',
    }, { protect: true });

    const webAcl = new aws.wafv2.WebAcl(`${serviceName}-acl`, {
      defaultAction: {
        allow: {},
      },
      name: `${serviceName}-acl`,
      description: `WebACL for order stream service ${chainId}`,
      rules: [
        // IP Reputation - AWS managed
        {
          name: 'ip-rep',
          priority: 1,
          statement: {
            managedRuleGroupStatement: {
              name: 'AWSManagedRulesAmazonIpReputationList',
              vendorName: 'AWS',
            },
          },
          overrideAction: {
            none: {},
          },
          visibilityConfig: {
            cloudwatchMetricsEnabled: true,
            metricName: 'ip-rep-rule',
            sampledRequestsEnabled: true,
          },
        },
        // Rate limiter by IP
        {
          name: 'rate-limit',
          priority: 2,
          action: {
            block: {},
          },
          statement: {
            rateBasedStatement: {
              aggregateKeyType: 'IP',
              limit: 250,
            },
          },
          visibilityConfig: {
            cloudwatchMetricsEnabled: true,
            metricName: 'rate-limit-rule',
            sampledRequestsEnabled: true,
          },
        },
      ],
      scope: 'REGIONAL',
      visibilityConfig: {
        cloudwatchMetricsEnabled: true,
        metricName: `${serviceName}-waf`,
        sampledRequestsEnabled: true,
      },
    });

    new aws.wafv2.WebAclAssociation(`${serviceName}-wacl-assoc`, {
      resourceArn: loadbalancer.loadBalancer.arn,
      webAclArn: webAcl.arn,
    });

    const dbUrlSecret = new aws.secretsmanager.Secret(`${serviceName}-db-url`);
    new aws.secretsmanager.SecretVersion(`${serviceName}-db-url-ver`, {
      secretId: dbUrlSecret.id,
      secretString: pulumi.interpolate`postgres://${rdsUser}:${rdsPassword}@${rds.address}:${rdsPort}/${rdsDbName}?sslmode=require`,
    });

    const dbSecretAccessPolicy = new aws.iam.Policy(`${serviceName}-db-url-policy`, {
      policy: dbUrlSecret.arn.apply((secretArn): aws.iam.PolicyDocument => {
        return {
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
              Resource: [secretArn],
            },
          ],
        };
      }),
    });

    const executionRole = new aws.iam.Role(`${serviceName}-ecs-execution-role`, {
      assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
        Service: 'ecs-tasks.amazonaws.com',
      }),
    });

    ecrRepository.repository.arn.apply(arn => {
      new aws.iam.RolePolicy(`${serviceName}-ecs-execution-pol`, {
        role: executionRole.id,
        policy: {
          Version: '2012-10-17',
          Statement: [
            {
              Effect: 'Allow',
              Action: [
                'ecr:GetAuthorizationToken',
                'ecr:BatchCheckLayerAvailability',
                'ecr:GetDownloadUrlForLayer',
                'ecr:BatchGetImage',
              ],
              Resource: '*',
            },
            {
              Effect: 'Allow',
              Action: [
                'logs:CreateLogStream',
                'logs:PutLogEvents',
              ],
              Resource: '*',
            },
            {
              Effect: 'Allow',
              Action: ['secretsmanager:GetSecretValue', 'ssm:GetParameters'],
              Resource: [dbUrlSecret.arn],
            },
          ],
        },
      });
    })

    const cluster = new aws.ecs.Cluster(`${serviceName}-cluster`, {
      name: `${serviceName}-cluster`,
    });

    const serviceLogGroup = `${serviceName}`;

    const albEndPoint = pulumi.interpolate`http://${loadbalancer.loadBalancer.dnsName}/`;
    const domain = albDomain ?? loadbalancer.loadBalancer.dnsName;

    const service = new awsx.ecs.FargateService(`${serviceName}-serv`, {
      name: `${serviceName}-serv`,
      cluster: cluster.arn,
      networkConfiguration: {
        securityGroups: [orderStreamSecGroup.id],
        assignPublicIp: false,
        subnets: privSubNetIds,
      },
      desiredCount: 1,
      deploymentCircuitBreaker: {
        enable: true,
        rollback: false,
      },
      // forceDelete: true,
      forceNewDeployment: true,
      enableExecuteCommand: true,
      taskDefinitionArgs: {
        logGroup: {
          args: {
            name: serviceLogGroup,
            retentionInDays: 0,
            skipDestroy: true,
          },
        },
        executionRole: { roleArn: executionRole.arn },
        taskRole: {
          args: {
            name: `${serviceName}-task`,
            description: 'order stream ECS task role with db secret access',
            managedPolicyArns: [dbSecretAccessPolicy.arn],
          },
        },
        container: {
          name: `${serviceName}`,
          image: image.ref,
          cpu: 1024,
          memory: 512,
          essential: true,
          linuxParameters: {
            initProcessEnabled: true,
          },
          command: [
            '--rpc-url',
            ethRpcUrl,
            '--boundless-market-address',
            boundlessAddress,
            '--min-balance',
            minBalance,
            '--bypass-addrs',
            bypassAddrs,
            '--domain',
            domain,
            '--ping-time',
            orderStreamPingTime.toString(),
          ],
          secrets: [
            {
              name: 'DATABASE_URL',
              valueFrom: dbUrlSecret.arn,
            },
          ],
          environment: [
            {
              name: 'RUST_LOG',
              value: 'order_stream=debug,tower_http=debug,info',
            },
            {
              name: 'NO_COLOR',
              value: '1',
            },
            {
              name: 'RUST_BACKTRACE',
              value: '1',
            },
            {
              name: 'DB_POOL_SIZE',
              value: '5',
            },
          ],
          healthCheck: {
            command: ['CMD-SHELL', `curl -f http://localhost:8585${HEALTH_CHECK_PATH} || exit 1`],
            interval: 60,
            timeout: 5,
            retries: 1,
            startPeriod: 5,
          },
          portMappings: [
            {
              containerPort: 8585,
              targetGroup: loadbalancer.defaultTargetGroup,
            },
          ],
        },
      },
    });

    const alarmActions = args.boundlessAlertsTopicArn ? [args.boundlessAlertsTopicArn] : [];

    new aws.cloudwatch.LogMetricFilter(`${serviceName}-log-err-filter`, {
      name: `${serviceName}-log-err-filter`,
      logGroupName: serviceLogGroup,
      metricTransformation: {
        namespace: 'Boundless/Services/OrderStream',
        name: `${serviceName}-log-err`,
        value: '1',
        defaultValue: '0',
      },
      // Whitespace prevents us from alerting on SQL injection probes.
      pattern: `"ERROR "`,
    }, { dependsOn: [service] });

    // Two errors within an hour triggers alarm.
    new aws.cloudwatch.MetricAlarm(`${serviceName}-error-alarm`, {
      name: `${serviceName}-log-err`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: `Boundless/Services/${serviceName}`,
            metricName: `${serviceName}-log-err`,
            period: 60,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 1,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      // Two errors within an hour triggers alarm.
      evaluationPeriods: 60,
      datapointsToAlarm: 2,
      treatMissingData: 'notBreaching',
      alarmDescription: 'Order stream log ERROR level',
      actionsEnabled: true,
      alarmActions,
    });

    // Convert the arns to the format expected by the Cloudwatch metric alarm.
    // The format is of form:
    //  app/order-stream-11155111-lb/a1fb2124f59f54fb
    // and
    //  targetgroup/order-stream-11155111-tg/6f6f9bce2553bf09
    const loadBalancerId = pulumi.interpolate`${loadbalancer.loadBalancer.arn.apply((arn) => arn.split('/').pop())}`;
    const targetGroupId = pulumi.interpolate`targetgroup/${loadbalancer.defaultTargetGroup.arn.apply((arn) => arn.split('/').pop())}`;

    new aws.cloudwatch.MetricAlarm(`${serviceName}-health-check-alarm`, {
      name: `${serviceName}-health-check-alarm`,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 1,
      metricName: `UnHealthyHostCount`,
      dimensions: {
        TargetGroup: targetGroupId,
        LoadBalancer: loadBalancerId,
      },
      namespace: 'AWS/ApplicationELB',
      period: 60,
      statistic: 'Sum',
      threshold: 1,
      alarmDescription: 'Order stream health check alarm',
      actionsEnabled: true,
      alarmActions,
    });

    this.lbUrl = albEndPoint;
    this.swaggerUrl = domain.apply((domain) => {
      return albDomain ? `https://${domain}/swagger-ui` : `http://${domain}/swagger-ui`;
    });
  }
}
