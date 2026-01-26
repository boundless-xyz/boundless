import * as path from 'path';
import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import * as crypto from 'crypto';
import { createRustLambda } from './rust-lambda';
import { Severity } from '../../util';

// WAF rate-based rules are evaluated per 5-minute window.
const RATE_LIMIT_PER_5_MINUTES = 200;
const ALLOWED_IP_RATE_LIMIT_PER_5_MINUTES = 1000;
// Header names must be lowercase for WAFv2 `singleHeader` matching.
const PROXY_SECRET_HEADER_NAME = 'x-boundless-proxy-secret';
const FORWARDED_IP_HEADER_NAME = 'x-forwarded-for';

export interface IndexerApiArgs {
  /** VPC where RDS lives */
  vpcId: pulumi.Input<string>;
  /** Private subnets for Lambda to attach to */
  privSubNetIds: pulumi.Input<pulumi.Input<string>[]>;
  /** RDS Reader Url secret (for read-only queries) */
  dbReaderUrlSecret: aws.secretsmanager.Secret;
  /** Hash of DB secrets to trigger Lambda updates when secrets change */
  secretHash: pulumi.Output<string>;
  /** RDS sg ID */
  rdsSgId: pulumi.Input<string>;
  /** Indexer Security Group ID (that has access to RDS) */
  indexerSgId: pulumi.Input<string>;
  /** RUST_LOG level */
  rustLogLevel: string;
  /** Chain ID */
  chainId: pulumi.Input<string>;
  /** Optional custom domain for CloudFront */
  domain?: pulumi.Input<string>;
  /** Proxy secret value used to decide whether to trust forwarded client IP headers */
  proxySecret: pulumi.Input<string>;
  /** Boundless alerts topic ARNs */
  boundlessAlertsTopicArns?: string[];
  /** Database version */
  databaseVersion?: string;
  /** Optional comma-separated list of IP addresses to allow higher rate limit (1000 per 5 minutes) */
  allowedIpAddresses?: pulumi.Input<string>;
}

export class IndexerApi extends pulumi.ComponentResource {
  public readonly lambdaFunction: aws.lambda.Function;
  public readonly apiEndpoint: pulumi.Output<string>;
  public readonly apiGatewayId: pulumi.Output<string>;
  public readonly logGroupName: pulumi.Output<string>;
  public readonly cloudFrontDomain: pulumi.Output<string>;
  public readonly distributionId: pulumi.Output<string>;

  constructor(
    name: string,
    args: IndexerApiArgs,
    opts?: pulumi.ComponentResourceOptions,
  ) {
    super(name, name, opts);

    const serviceName = name;

    const usEast1Provider = new aws.Provider(
      `${serviceName}-us-east-1`,
      { region: 'us-east-1' },
      { parent: this },
    );

    // Create IAM role for Lambda
    const role = new aws.iam.Role(
      `${serviceName}-role`,
      {
        assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({ Service: 'lambda.amazonaws.com' }),
      },
      { parent: this },
    );

    // Attach basic execution role policy
    new aws.iam.RolePolicyAttachment(
      `${serviceName}-logs`,
      {
        role: role.name,
        policyArn: aws.iam.ManagedPolicies.AWSLambdaBasicExecutionRole,
      },
      { parent: this },
    );

    // Attach VPC access policy
    new aws.iam.RolePolicyAttachment(
      `${serviceName}-vpc-access`,
      {
        role: role.name,
        policyArn: aws.iam.ManagedPolicies.AWSLambdaVPCAccessExecutionRole,
      },
      { parent: this },
    );

    // Create inline policy for Secrets Manager access
    const inlinePolicy = pulumi.all([args.dbReaderUrlSecret.arn]).apply(([secretArn]) =>
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


    const dbUrl = pulumi.all([args.dbReaderUrlSecret]).apply(([secret]) => {
      // Get database URL from secret (reader endpoint for read-only queries)
      const dbUrl = aws.secretsmanager.getSecretVersionOutput({
        secretId: secret.id,
      }).secretString;
      return dbUrl;
    });

    // Combine secret hash from infra with local env variables
    // So that we can trigger a Lambda update when the secrets change
    const envHash = pulumi.all([dbUrl, args.secretHash]).apply(([dbUrl, infraSecretHash]) => {
      const hash = crypto.createHash('sha1');
      hash.update(dbUrl);
      hash.update(infraSecretHash);
      hash.update(args.rustLogLevel);
      return hash.digest('hex');
    });


    // Create the Lambda function
    const { lambda, logGroupName } = createRustLambda(`${serviceName}-lambda`, {
      projectPath: path.join(__dirname, '../../../'),
      packageName: 'indexer-api',
      release: true,
      nameSuffix: args.databaseVersion ?? '',
      role: role.arn,
      environmentVariables: {
        DB_URL: dbUrl,
        RUST_LOG: args.rustLogLevel,
        SECRET_HASH: envHash,
        CHAIN_ID: pulumi.output(args.chainId),
      },
      memorySize: 256,
      timeout: 30,
      vpcConfig: {
        subnetIds: args.privSubNetIds,
        securityGroupIds: [args.indexerSgId],
      },
    });

    this.lambdaFunction = lambda;
    this.logGroupName = logGroupName;

    // Create error log metric filter and alarm
    const alarmActions = args.boundlessAlertsTopicArns ?? [];

    const errorLogMetricName = `${serviceName}-log-err`;
    new aws.cloudwatch.LogMetricFilter(`${serviceName}-error-filter`, {
      name: `${serviceName}-log-err-filter`,
      logGroupName: logGroupName,
      metricTransformation: {
        namespace: `Boundless/Services/${serviceName}`,
        name: errorLogMetricName,
        value: '1',
        defaultValue: '0',
      },
      pattern: '?ERROR ?error ?Error',
    }, { dependsOn: [this.lambdaFunction], parent: this });

    const stackName = pulumi.getStack();
    const stage = stackName.includes('prod') ? 'prod' : 'staging';
    const chainIdOutput = pulumi.output(args.chainId);
    const alarmTags = pulumi.all([chainIdOutput, logGroupName]).apply(([chainId, logGroup]) => ({
      StackName: stage,
      ChainId: chainId,
      ServiceName: serviceName,
      LogGroupName: logGroup,
    }));

    new aws.cloudwatch.MetricAlarm(`${serviceName}-${Severity.SEV2}-error-alarm`, {
      name: `${serviceName}-${Severity.SEV2}-log-err`,
      metricQueries: [
        {
          id: 'm1',
          metric: {
            namespace: `Boundless/Services/${serviceName}`,
            metricName: errorLogMetricName,
            period: 300,
            stat: 'Sum',
          },
          returnData: true,
        },
      ],
      threshold: 2,
      comparisonOperator: 'GreaterThanOrEqualToThreshold',
      evaluationPeriods: 4,
      datapointsToAlarm: 2,
      treatMissingData: 'notBreaching',
      alarmDescription: `Indexer API Lambda (${serviceName}) ${Severity.SEV2} log ERROR level (2 errors within 20 mins)`,
      actionsEnabled: true,
      alarmActions,
      tags: alarmTags,
    }, { parent: this });

    // Create API Gateway v2 (HTTP API)
    const api = new aws.apigatewayv2.Api(
      `${serviceName}-api`,
      {
        name: serviceName,
        protocolType: 'HTTP',
        corsConfiguration: {
          allowOrigins: ['*'],
          allowMethods: ['GET', 'OPTIONS'],
          allowHeaders: ['content-type', 'x-amz-date', 'authorization', 'x-api-key', 'x-amz-security-token'],
          exposeHeaders: ['x-amzn-RequestId'],
          maxAge: 300,
        },
      },
      { parent: this },
    );

    this.apiGatewayId = api.id;

    // Create Lambda integration
    const integration = new aws.apigatewayv2.Integration(
      `${serviceName}-integration`,
      {
        apiId: api.id,
        integrationType: 'AWS_PROXY',
        integrationUri: lambda.arn,
        integrationMethod: 'POST',
        payloadFormatVersion: '2.0',
      },
      { parent: this },
    );

    // Create route for all paths (Lambda will handle routing internally)
    new aws.apigatewayv2.Route(
      `${serviceName}-route`,
      {
        apiId: api.id,
        routeKey: '$default',
        target: pulumi.interpolate`integrations/${integration.id}`,
      },
      { parent: this },
    );

    // Create CloudWatch log group for API Gateway access logs
    const accessLogGroup = new aws.cloudwatch.LogGroup(
      `${serviceName}-api-access-logs`,
      {
        name: `/aws/apigateway/${serviceName}`,
        retentionInDays: 30,
      },
      { parent: this },
    );

    // Create deployment stage with access logging
    new aws.apigatewayv2.Stage(
      `${serviceName}-stage`,
      {
        apiId: api.id,
        name: '$default',
        autoDeploy: true,
        accessLogSettings: {
          destinationArn: accessLogGroup.arn,
          format: JSON.stringify({
            requestId: '$context.requestId',
            ip: '$context.identity.sourceIp',
            requestTime: '$context.requestTime',
            httpMethod: '$context.httpMethod',
            path: '$context.path',
            status: '$context.status',
            responseLength: '$context.responseLength',
            integrationLatency: '$context.integrationLatency',
            integrationError: '$context.integrationErrorMessage',
          }),
        },
      },
      { parent: this },
    );

    // Alarm for API Gateway 5xx errors
    new aws.cloudwatch.MetricAlarm(`${serviceName}-5xx-alarm`, {
      name: `${serviceName}-5xx-errors`,
      comparisonOperator: 'GreaterThanThreshold',
      evaluationPeriods: 2,
      metricName: '5xx',
      namespace: 'AWS/ApiGateway',
      period: 60,
      statistic: 'Sum',
      threshold: 10,
      alarmDescription: `API Gateway 5xx errors detected for ${serviceName}`,
      dimensions: {
        ApiId: api.id,
      },
      treatMissingData: 'notBreaching',
      alarmActions,
    }, { parent: this });

    this.apiEndpoint = pulumi.interpolate`${api.apiEndpoint}`;

    // Grant API Gateway permission to invoke Lambda
    new aws.lambda.Permission(
      `${serviceName}-api-permission`,
      {
        function: lambda.name,
        statementId: 'AllowAPIGatewayInvoke',
        action: 'lambda:InvokeFunction',
        principal: 'apigateway.amazonaws.com',
        sourceArn: pulumi.interpolate`${api.executionArn}/*`,
      },
      { parent: this },
    );


    let certificateArn: pulumi.Output<string> | undefined;
    let certificateValidation: aws.acm.CertificateValidation | undefined;
    let certificateValidationRecords: pulumi.Output<{ name: string; value: string; type: string }[]> | undefined;
    let distributionAliases: pulumi.Input<pulumi.Input<string>[]> | undefined;

    if (args.domain) {
      const certificate = new aws.acm.Certificate(
        `${serviceName}-cert`,
        {
          domainName: args.domain,
          validationMethod: 'DNS',
        },
        { parent: this, provider: usEast1Provider },
      );

      certificateArn = certificate.arn;
      certificateValidationRecords = certificate.domainValidationOptions.apply(options =>
        options.map(option => ({
          name: option.resourceRecordName,
          value: option.resourceRecordValue,
          type: option.resourceRecordType,
        })),
      );

      certificateValidation = new aws.acm.CertificateValidation(
        `${serviceName}-cert-validation`,
        {
          certificateArn: certificate.arn,
          validationRecordFqdns: certificate.domainValidationOptions.apply(options =>
            options.map(option => option.resourceRecordName),
          ),
        },
        { parent: this, provider: usEast1Provider },
      );

      distributionAliases = [args.domain];
    }

    // Parse comma-separated IP addresses and always create IP set
    const parsedIpAddresses = args.allowedIpAddresses
      ? pulumi.output(args.allowedIpAddresses).apply(ips =>
        ips.split(',').map(ip => ip.trim()).filter(ip => ip.length > 0)
      )
      : pulumi.output([]);

    const allowedIpSet = parsedIpAddresses.apply(ips =>
      new aws.wafv2.IpSet(
        `${serviceName}-allowed-ip`,
        {
          name: `${serviceName}-allowed-ip`,
          scope: 'CLOUDFRONT',
          addresses: ips,
          ipAddressVersion: 'IPV4',
        },
        { parent: this, provider: usEast1Provider },
      )
    );

    // Build WAF rules array - always include allowed IP rule at priority 0
    const allRules = allowedIpSet.apply(ipSet => [
      {
        name: 'AllowedIpRateLimit',
        priority: 0,
        statement: {
          rateBasedStatement: {
            limit: ALLOWED_IP_RATE_LIMIT_PER_5_MINUTES,
            aggregateKeyType: 'IP',
            scopeDownStatement: {
              ipSetReferenceStatement: {
                arn: ipSet.arn,
              },
            },
          },
        },
        action: { block: {} },
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: 'AllowedIpRateLimit',
        },
      },
      // If proxy secret matches, rate limit by forwarded client IP (from X-Forwarded-For)
      {
        name: 'ProxyForwardedIpRateLimit',
        priority: 1,
        statement: {
          rateBasedStatement: {
            limit: RATE_LIMIT_PER_5_MINUTES,
            aggregateKeyType: 'FORWARDED_IP',
            forwardedIpConfig: {
              headerName: FORWARDED_IP_HEADER_NAME,
              fallbackBehavior: 'NO_MATCH',
            },
            scopeDownStatement: {
              byteMatchStatement: {
                searchString: args.proxySecret,
                fieldToMatch: {
                  singleHeader: { name: PROXY_SECRET_HEADER_NAME },
                },
                positionalConstraint: 'EXACTLY',
                textTransformations: [{ priority: 0, type: 'NONE' }],
              },
            },
          },
        },
        action: { block: {} },
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: 'ProxyForwardedIpRateLimit',
        },
      },
      // If proxy secret does NOT match, rate limit by true source IP
      {
        name: 'SourceIpRateLimit',
        priority: 2,
        statement: {
          rateBasedStatement: {
            limit: RATE_LIMIT_PER_5_MINUTES,
            aggregateKeyType: 'IP',
            scopeDownStatement: {
              notStatement: {
                statements: [
                  {
                    byteMatchStatement: {
                      searchString: args.proxySecret,
                      fieldToMatch: {
                        singleHeader: { name: PROXY_SECRET_HEADER_NAME },
                      },
                      positionalConstraint: 'EXACTLY',
                      textTransformations: [{ priority: 0, type: 'NONE' }],
                    },
                  },
                ],
              },
            },
          },
        },
        action: { block: {} },
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: 'SourceIpRateLimit',
        },
      },
      // AWS Managed Core Rule Set
      {
        name: 'AWS-AWSManagedRulesCommonRuleSet',
        priority: 3,
        overrideAction: {
          none: {},
        },
        statement: {
          managedRuleGroupStatement: {
            vendorName: 'AWS',
            name: 'AWSManagedRulesCommonRuleSet',
          },
        },
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: 'AWSManagedRulesCommonRuleSetMetric',
        },
      },
      // AWS Managed Known Bad Inputs Rule Set
      {
        name: 'AWS-AWSManagedRulesKnownBadInputsRuleSet',
        priority: 4,
        overrideAction: {
          none: {},
        },
        statement: {
          managedRuleGroupStatement: {
            vendorName: 'AWS',
            name: 'AWSManagedRulesKnownBadInputsRuleSet',
          },
        },
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: 'AWSManagedRulesKnownBadInputsRuleSetMetric',
        },
      },
      // SQL Injection Protection
      {
        name: 'AWS-AWSManagedRulesSQLiRuleSet',
        priority: 5,
        overrideAction: {
          none: {},
        },
        statement: {
          managedRuleGroupStatement: {
            vendorName: 'AWS',
            name: 'AWSManagedRulesSQLiRuleSet',
          },
        },
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: 'AWSManagedRulesSQLiRuleSetMetric',
        },
      },
    ]);

    // Create WAF WebACL
    const webAcl = new aws.wafv2.WebAcl(
      `${serviceName}-waf`,
      {
        name: `${serviceName}-waf`,
        scope: 'CLOUDFRONT',
        defaultAction: {
          allow: {},
        },
        rules: allRules,
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: `${serviceName}-waf`,
        },
      },
      {
        parent: this,
        provider: usEast1Provider,
        dependsOn: [allowedIpSet]
      }, // WAF for CloudFront must be in us-east-1
    );

    // Cache policy for default behavior: short TTL (60s default, 300s max)
    const shortCachePolicy = new aws.cloudfront.CachePolicy(
      `${serviceName}-short-cache`,
      {
        name: `${serviceName}-short-cache`,
        comment: 'Short cache for API responses (60s default)',
        defaultTtl: 60,
        minTtl: 0,
        maxTtl: 300,
        parametersInCacheKeyAndForwardedToOrigin: {
          cookiesConfig: {
            cookieBehavior: 'none',
          },
          headersConfig: {
            headerBehavior: 'none',
          },
          queryStringsConfig: {
            queryStringBehavior: 'all',
          },
          enableAcceptEncodingGzip: true,
          enableAcceptEncodingBrotli: true,
        },
      },
      { parent: this },
    );

    // Cache policy for epoch leaderboard: longer TTL (5m default, 1h max)
    const epochLeaderboardCachePolicy = new aws.cloudfront.CachePolicy(
      `${serviceName}-long-cache`,
      {
        name: `${serviceName}-long-cache`,
        comment: 'Longer cache for historical epoch data (5m default)',
        defaultTtl: 300,
        minTtl: 60,
        maxTtl: 3600,
        parametersInCacheKeyAndForwardedToOrigin: {
          cookiesConfig: {
            cookieBehavior: 'none',
          },
          headersConfig: {
            headerBehavior: 'none',
          },
          queryStringsConfig: {
            queryStringBehavior: 'all',
          },
          enableAcceptEncodingGzip: true,
          enableAcceptEncodingBrotli: true,
        },
      },
      { parent: this },
    );

    // Origin request policy: forward all query strings to origin
    const originRequestPolicy = new aws.cloudfront.OriginRequestPolicy(
      `${serviceName}-origin-request`,
      {
        name: `${serviceName}-origin-request`,
        comment: 'Forward all query strings to origin',
        cookiesConfig: {
          cookieBehavior: 'none',
        },
        headersConfig: {
          headerBehavior: 'none',
        },
        queryStringsConfig: {
          queryStringBehavior: 'all',
        },
      },
      { parent: this },
    );

    // Parse API endpoint to get domain
    const apiDomain = this.apiEndpoint.apply(endpoint => {
      const url = new URL(endpoint);
      return url.hostname;
    });

    const viewerCertificate: pulumi.Input<aws.types.input.cloudfront.DistributionViewerCertificate> =
      certificateValidation
        ? {
          acmCertificateArn: certificateValidation.certificateArn,
          sslSupportMethod: 'sni-only',
          minimumProtocolVersion: 'TLSv1.2_2021',
        }
        : {
          cloudfrontDefaultCertificate: true,
        };

    const distributionOpts: pulumi.CustomResourceOptions = { parent: this };
    if (certificateValidation) {
      distributionOpts.dependsOn = [certificateValidation];
    }

    // Create CloudFront distribution
    const distribution = new aws.cloudfront.Distribution(
      `${serviceName}-cdn`,
      {
        enabled: true,
        isIpv6Enabled: true,
        comment: `${serviceName} API CDN`,
        priceClass: 'PriceClass_100', // Use only North America and Europe edge locations
        webAclId: webAcl.arn,
        aliases: distributionAliases,

        origins: [{
          domainName: apiDomain,
          originId: 'api',
          customOriginConfig: {
            httpPort: 80,
            httpsPort: 443,
            originProtocolPolicy: 'https-only',
            originSslProtocols: ['TLSv1.2'],
          },
        }],

        defaultCacheBehavior: {
          targetOriginId: 'api',
          viewerProtocolPolicy: 'redirect-to-https',
          allowedMethods: ['GET', 'HEAD', 'OPTIONS'],
          cachedMethods: ['GET', 'HEAD', 'OPTIONS'],
          compress: true,
          cachePolicyId: shortCachePolicy.id,
          originRequestPolicyId: originRequestPolicy.id,
        },

        orderedCacheBehaviors: [
          {
            // Historical epoch data - cache longer
            pathPattern: '/v1/rewards/povw/leaderboard/epoch/*',
            targetOriginId: 'api',
            viewerProtocolPolicy: 'redirect-to-https',
            allowedMethods: ['GET', 'HEAD', 'OPTIONS'],
            cachedMethods: ['GET', 'HEAD', 'OPTIONS'],
            compress: true,
            cachePolicyId: epochLeaderboardCachePolicy.id,
            originRequestPolicyId: originRequestPolicy.id,
          },
        ],

        restrictions: {
          geoRestriction: {
            restrictionType: 'none',
          },
        },

        viewerCertificate,

        customErrorResponses: [
          {
            errorCode: 500,
            errorCachingMinTtl: 0,
          },
          {
            errorCode: 502,
            errorCachingMinTtl: 0,
          },
          {
            errorCode: 503,
            errorCachingMinTtl: 0,
          },
          {
            errorCode: 504,
            errorCachingMinTtl: 0,
          },
        ],
      },
      distributionOpts,
    );

    this.cloudFrontDomain = distribution.domainName;
    this.distributionId = distribution.id;

    const componentOutputs: Record<string, pulumi.Input<any>> = {
      lambdaFunction: lambda.id,
      apiEndpoint: this.apiEndpoint,
      apiGatewayId: this.apiGatewayId,
      logGroupName: this.logGroupName,
      cloudFrontDomain: this.cloudFrontDomain,
      distributionId: this.distributionId,
    };

    if (certificateArn) {
      componentOutputs.certificateArn = certificateArn;
    }

    if (certificateValidationRecords) {
      componentOutputs.certificateValidationRecords = certificateValidationRecords;
    }

    this.registerOutputs(componentOutputs);
  }
}
