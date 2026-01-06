import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';
import { getEnvVar, getServiceNameV1 } from '../util';

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const bucketName = config.get('BUCKET_NAME') || `boundless-requestor-lists-${stackName}`;
  const domain = config.get('DOMAIN');
  const serviceName = getServiceNameV1(stackName, "requestor-lists");

  // Create an S3 bucket for hosting the requestor lists
  const bucket = new aws.s3.Bucket(`${serviceName}-bucket`, {
    bucket: bucketName,
    corsRules: [
      {
        allowedHeaders: ['*'],
        allowedMethods: ['GET', 'HEAD'],
        allowedOrigins: ['*'],
        exposeHeaders: ['ETag'],
        maxAgeSeconds: 3000,
      },
    ],
  });

  const bucketOwnershipControls = new aws.s3.BucketOwnershipControls(`${serviceName}-ownership-controls`, {
    bucket: bucket.id,
    rule: {
      objectOwnership: "BucketOwnerEnforced",
    },
  });

  const bucketPublicAccessBlock = new aws.s3.BucketPublicAccessBlock(
    `${serviceName}-public-access-block`,
    {
      bucket: bucket.id,
      blockPublicAcls: false,
      blockPublicPolicy: false,
      ignorePublicAcls: false,
      restrictPublicBuckets: false,
    }
  );

  // Bucket policy - always allow public read, plus CloudFront OAI if domain is set
  let originAccessIdentity: aws.cloudfront.OriginAccessIdentity | undefined;

  if (domain) {
    // Create CloudFront Origin Access Identity
    originAccessIdentity = new aws.cloudfront.OriginAccessIdentity(`${serviceName}-oai`, {
      comment: `OAI for ${domain}`,
    });
  }

  // Always allow public read access
  const bucketPolicy = new aws.s3.BucketPolicy(
    `${serviceName}-bucket-policy`,
    {
      bucket: bucket.id,
      policy: domain
        ? pulumi.all([bucket.arn, originAccessIdentity!.iamArn]).apply(([arn, oaiArn]: [string, string]) =>
          pulumi.jsonStringify({
            Version: '2012-10-17',
            Statement: [
              {
                Sid: 'PublicReadGetObject',
                Effect: 'Allow',
                Principal: '*',
                Action: 's3:GetObject',
                Resource: pulumi.interpolate`${arn}/*`,
              },
              {
                Sid: 'CloudFrontReadGetObject',
                Effect: 'Allow',
                Principal: {
                  AWS: oaiArn,
                },
                Action: 's3:GetObject',
                Resource: pulumi.interpolate`${arn}/*`,
              },
            ],
          })
        )
        : pulumi.jsonStringify({
          Version: '2012-10-17',
          Statement: [
            {
              Sid: 'PublicReadGetObject',
              Effect: 'Allow',
              Principal: '*',
              Action: 's3:GetObject',
              Resource: pulumi.interpolate`${bucket.arn}/*`,
            },
          ],
        }),
    },
    {
      dependsOn: [
        bucketOwnershipControls,
        bucketPublicAccessBlock,
      ],
    }
  );

  // Upload the JSON files
  new aws.s3.BucketObject(
    `${serviceName}-boundless-recommended-priority-list`,
    {
      bucket: bucket.id,
      key: 'boundless-recommended-priority-list.standard.json',
      source: new pulumi.asset.FileAsset('../../requestor-lists/boundless-priority-list.standard.json'),
      contentType: 'application/json',
      cacheControl: 'no-cache, no-store, must-revalidate',
    },
    { dependsOn: [bucketPolicy] }
  );

  new aws.s3.BucketObject(
    `${serviceName}-boundless-recommended-priority-list-large`,
    {
      bucket: bucket.id,
      key: 'boundless-recommended-priority-list.large.json',
      source: new pulumi.asset.FileAsset('../../requestor-lists/boundless-priority-list.large.json'),
      contentType: 'application/json',
      cacheControl: 'no-cache, no-store, must-revalidate',
    },
    { dependsOn: [bucketPolicy] }
  );

  new aws.s3.BucketObject(
    `${serviceName}-boundless-recommended-priority-list-extra-large`,
    {
      bucket: bucket.id,
      key: 'boundless-recommended-priority-list.extra-large.json',
      source: new pulumi.asset.FileAsset('../../requestor-lists/boundless-priority-list.extra-large.json'),
      contentType: 'application/json',
      cacheControl: 'no-cache, no-store, must-revalidate',
    },
    { dependsOn: [bucketPolicy] }
  );

  // Upload the allowed list JSON files
  new aws.s3.BucketObject(
    `${serviceName}-boundless-allowed-list`,
    {
      bucket: bucket.id,
      key: 'boundless-allowed-list.json',
      source: new pulumi.asset.FileAsset('../../requestor-lists/boundless-allowed-list.json'),
      contentType: 'application/json',
      cacheControl: 'no-cache, no-store, must-revalidate',
    },
    { dependsOn: [bucketPolicy] }
  );

  let outputs: any = {
    bucketName: bucket.id,
    listUrl: pulumi.interpolate`https://${bucket.bucketRegionalDomainName}/boundless-recommended-priority-list.standard.json`,
    listUrlLarge: pulumi.interpolate`https://${bucket.bucketRegionalDomainName}/boundless-recommended-priority-list.large.json`,
    listUrlExtraLarge: pulumi.interpolate`https://${bucket.bucketRegionalDomainName}/boundless-recommended-priority-list.extra-large.json`,
    allowedListUrl: pulumi.interpolate`https://${bucket.bucketRegionalDomainName}/boundless-allowed-list.json`,
  };

  // If custom domain is configured, create ACM cert and CloudFront distribution
  if (domain) {
    const usEast1Provider = new aws.Provider('us-east-1-provider', { region: 'us-east-1' });

    // Create ACM certificate for the custom domain (must be in us-east-1 for CloudFront)
    const certificate = new aws.acm.Certificate(
      `${serviceName}-cert`,
      {
        domainName: domain,
        validationMethod: 'DNS',
        tags: {
          Name: `Certificate for ${domain}`,
          Environment: stackName,
        },
      },
      { provider: usEast1Provider }
    );

    // Create certificate validation resource
    const certificateValidation = new aws.acm.CertificateValidation(
      `${serviceName}-cert-validation`,
      {
        certificateArn: certificate.arn,
        validationRecordFqdns: certificate.domainValidationOptions.apply(options =>
          options.map(option => option.resourceRecordName),
        ),
      },
      { provider: usEast1Provider }
    );

    // Create CloudFront distribution
    const distribution = new aws.cloudfront.Distribution(`${serviceName}-cdn`, {
      enabled: true,
      aliases: [domain],
      origins: [
        {
          domainName: bucket.bucketRegionalDomainName,
          originId: 'S3-requestor-lists',
          s3OriginConfig: {
            originAccessIdentity: originAccessIdentity!.cloudfrontAccessIdentityPath,
          },
        },
      ],
      defaultRootObject: 'boundless-recommended-priority-list.standard.json',
      defaultCacheBehavior: {
        targetOriginId: 'S3-requestor-lists',
        viewerProtocolPolicy: 'redirect-to-https',
        allowedMethods: ['GET', 'HEAD', 'OPTIONS'],
        cachedMethods: ['GET', 'HEAD', 'OPTIONS'],
        forwardedValues: {
          queryString: false,
          headers: ['Origin', 'Access-Control-Request-Headers', 'Access-Control-Request-Method'],
          cookies: {
            forward: 'none',
          },
        },
        minTtl: 0,
        defaultTtl: 0,
        maxTtl: 0,
        compress: true,
      },
      restrictions: {
        geoRestriction: {
          restrictionType: 'none',
        },
      },
      viewerCertificate: {
        acmCertificateArn: certificateValidation.certificateArn,
        sslSupportMethod: 'sni-only',
        minimumProtocolVersion: 'TLSv1.2_2021',
      },
      tags: {
        Name: 'Boundless Requestor Lists CDN',
        Environment: stackName,
      },
    }, {
      dependsOn: [certificateValidation],
    });

    outputs = {
      ...outputs,
      domain,
      certificateArn: certificate.arn,
      certificateValidationRecords: certificate.domainValidationOptions,
      cloudfrontDomain: distribution.domainName,
      cloudfrontDistributionId: distribution.id,
      customDomainUrl: pulumi.interpolate`https://${domain}/boundless-recommended-priority-list.standard.json`,
      customDomainUrlLarge: pulumi.interpolate`https://${domain}/boundless-recommended-priority-list.large.json`,
      customDomainUrlExtraLarge: pulumi.interpolate`https://${domain}/boundless-recommended-priority-list.extra-large.json`,
      allowedListCustomDomainUrl: pulumi.interpolate`https://${domain}/boundless-allowed-list.json`,
    };
  }

  return outputs;
};
