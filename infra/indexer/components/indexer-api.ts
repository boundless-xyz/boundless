import * as path from 'path';
import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { createRustLambda } from './rust-lambda';

export interface IndexerApiArgs {
  /** VPC where RDS lives */
  vpcId: pulumi.Input<string>;
  /** Private subnets for Lambda to attach to */
  privSubNetIds: pulumi.Input<pulumi.Input<string>[]>;
  /** RDS Url secret */
  dbUrlSecret: aws.secretsmanager.Secret;
  /** RDS sg ID */
  rdsSgId: pulumi.Input<string>;
  /** Indexer Security Group ID (that has access to RDS) */
  indexerSgId: pulumi.Input<string>;
  /** RUST_LOG level */
  rustLogLevel: string;
  /** Optional custom domain for CloudFront */
  domain?: pulumi.Input<string>;
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
    const inlinePolicy = pulumi.all([args.dbUrlSecret.arn]).apply(([secretArn]) =>
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

    // Use the existing indexer security group that already has access to RDS
    // This is the same security group used by the ECS tasks

    // Get database URL from secret
    const dbUrl = aws.secretsmanager.getSecretVersionOutput({
      secretId: args.dbUrlSecret.id,
    }).secretString;

    // Create the Lambda function
    const { lambda, logGroupName } = createRustLambda(`${serviceName}-lambda`, {
      projectPath: path.join(__dirname, '../../../'),
      packageName: 'indexer-api',
      release: true,
      role: role.arn,
      environmentVariables: {
        DB_URL: dbUrl,
        RUST_LOG: args.rustLogLevel,
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

    // Create deployment stage
    const apiStage = new aws.apigatewayv2.Stage(
      `${serviceName}-stage`,
      {
        apiId: api.id,
        name: '$default',
        autoDeploy: true,
      },
      { parent: this },
    );

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


    // Create WAF WebACL
    const webAcl = new aws.wafv2.WebAcl(
      `${serviceName}-waf`,
      {
        name: `${serviceName}-waf`,
        scope: 'CLOUDFRONT',
        defaultAction: {
          allow: {},
        },
        rules: [
          // Rate limiting rule
          {
            name: 'RateLimitRule',
            priority: 1,
            statement: {
              rateBasedStatement: {
                limit: 75, // 75 requests per 5 minutes per IP
                aggregateKeyType: 'IP',
                forwardedIpConfig: {
                  headerName: 'CF-Connecting-IP',
                  fallbackBehavior: 'MATCH',
                },
              },
            },
            action: {
              block: {},
            },
            visibilityConfig: {
              sampledRequestsEnabled: true,
              cloudwatchMetricsEnabled: true,
              metricName: 'RateLimitRule',
            },
          },
          // AWS Managed Core Rule Set
          {
            name: 'AWS-AWSManagedRulesCommonRuleSet',
            priority: 2,
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
            priority: 3,
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
            priority: 4,
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
        ],
        visibilityConfig: {
          sampledRequestsEnabled: true,
          cloudwatchMetricsEnabled: true,
          metricName: `${serviceName}-waf`,
        },
      },
      { parent: this, provider: usEast1Provider }, // WAF for CloudFront must be in us-east-1
    );

    // Parse API endpoint to get domain
    const apiDomain = this.apiEndpoint.apply(endpoint => {
      const url = new URL(endpoint);
      return url.hostname;
    });

    const viewerCertificate: pulumi.Input<aws.types.input.cloudfront.DistributionViewerCertificate> =
      certificateArn
        ? {
          acmCertificateArn: certificateArn,
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

          // Cache policy for default behavior (current leaderboard)
          defaultTtl: 60,    // 1 minute default
          minTtl: 0,         // Allow immediate expiration
          maxTtl: 300,       // Max 5 minutes

          forwardedValues: {
            queryString: true, // Forward query parameters for pagination
            cookies: {
              forward: 'none',
            },
            headers: [], // API Gateway doesn't need special headers
          },
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

            defaultTtl: 300,   // 5 minutes default
            minTtl: 60,        // At least 1 minute
            maxTtl: 3600,      // Max 1 hour

            forwardedValues: {
              queryString: true,
              cookies: {
                forward: 'none',
              },
              headers: [],
            },
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
            errorCode: 403,
            responseCode: 403,
            responsePagePath: '/error.html',
            errorCachingMinTtl: 10,
          },
          {
            errorCode: 404,
            responseCode: 404,
            responsePagePath: '/error.html',
            errorCachingMinTtl: 10,
          },
          {
            errorCode: 500,
            errorCachingMinTtl: 0, // Don't cache errors
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
      { parent: this },
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
