import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export = () => {
  const config = new pulumi.Config();
  const stackName = pulumi.getStack();
  const bucketName = config.get('BUCKET_NAME') || `boundless-requestor-lists-${stackName}`;
  const domain = config.get('DOMAIN');

  // Create an S3 bucket for hosting the requestor lists
  const bucket = new aws.s3.Bucket('requestor-lists-bucket', {
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
    tags: {
      Name: 'Boundless Requestor Priority Lists',
      Environment: stackName,
    },
  });

  // Block public access settings (we'll use CloudFront OAI if custom domain is set)
  const publicAccessBlock = new aws.s3.BucketPublicAccessBlock(
    'requestor-lists-public-access',
    {
      bucket: bucket.id,
      blockPublicAcls: !domain,
      blockPublicPolicy: !domain,
      ignorePublicAcls: !domain,
      restrictPublicBuckets: !domain,
    }
  );

  // Bucket policy - allow CloudFront OAI or public read
  let bucketPolicy: aws.s3.BucketPolicy | undefined;
  let originAccessIdentity: aws.cloudfront.OriginAccessIdentity | undefined;

  if (domain) {
    // Create CloudFront Origin Access Identity for private bucket access
    originAccessIdentity = new aws.cloudfront.OriginAccessIdentity('requestor-lists-oai', {
      comment: `OAI for ${domain}`,
    });

    bucketPolicy = new aws.s3.BucketPolicy(
      'requestor-lists-bucket-policy',
      {
        bucket: bucket.id,
        policy: pulumi.all([bucket.arn, originAccessIdentity.iamArn]).apply(([arn, oaiArn]: [string, string]) =>
          JSON.stringify({
            Version: '2012-10-17',
            Statement: [
              {
                Sid: 'CloudFrontReadGetObject',
                Effect: 'Allow',
                Principal: {
                  AWS: oaiArn,
                },
                Action: 's3:GetObject',
                Resource: `${arn}/*`,
              },
            ],
          })
        ),
      },
      { dependsOn: [publicAccessBlock] }
    );
  } else {
    // Public bucket policy for direct S3 access
    bucketPolicy = new aws.s3.BucketPolicy(
      'requestor-lists-bucket-policy',
      {
        bucket: bucket.id,
        policy: bucket.arn.apply((arn: string) =>
          JSON.stringify({
            Version: '2012-10-17',
            Statement: [
              {
                Sid: 'PublicReadGetObject',
                Effect: 'Allow',
                Principal: '*',
                Action: 's3:GetObject',
                Resource: `${arn}/*`,
              },
            ],
          })
        ),
      },
      { dependsOn: [publicAccessBlock] }
    );
  }

  // Upload the JSON file
  new aws.s3.BucketObject(
    'boundless-recommended-priority-list',
    {
      bucket: bucket.id,
      key: 'boundless-recommended-priority-list.json',
      source: new pulumi.asset.FileAsset('../../requestor-lists/boundless-recommended-priority-list.json'),
      contentType: 'application/json',
      acl: domain ? undefined : 'public-read',
    },
    { dependsOn: [bucketPolicy] }
  );

  let outputs: any = {
    bucketName: bucket.id,
    listUrl: pulumi.interpolate`https://${bucket.bucketDomainName}/boundless-recommended-priority-list.json`,
  };

  // If custom domain is configured, create ACM cert and CloudFront distribution
  if (domain) {
    // Create ACM certificate for the custom domain (must be in us-east-1 for CloudFront)
    const certificate = new aws.acm.Certificate(
      'requestor-lists-cert',
      {
        domainName: domain,
        validationMethod: 'DNS',
        tags: {
          Name: `Certificate for ${domain}`,
          Environment: stackName,
        },
      },
      { provider: new aws.Provider('us-east-1-provider', { region: 'us-east-1' }) }
    );

    // Create CloudFront distribution
    const distribution = new aws.cloudfront.Distribution('requestor-lists-cdn', {
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
      defaultRootObject: 'boundless-recommended-priority-list.json',
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
        defaultTtl: 3600,
        maxTtl: 86400,
        compress: true,
      },
      restrictions: {
        geoRestriction: {
          restrictionType: 'none',
        },
      },
      viewerCertificate: {
        acmCertificateArn: certificate.arn,
        sslSupportMethod: 'sni-only',
        minimumProtocolVersion: 'TLSv1.2_2021',
      },
      tags: {
        Name: 'Boundless Requestor Lists CDN',
        Environment: stackName,
      },
    });

    outputs = {
      ...outputs,
      domain,
      certificateArn: certificate.arn,
      certificateValidationRecords: certificate.domainValidationOptions,
      cloudfrontDomain: distribution.domainName,
      cloudfrontDistributionId: distribution.id,
      customDomainUrl: pulumi.interpolate`https://${domain}/boundless-recommended-priority-list.json`,
    };
  }

  return outputs;
};
