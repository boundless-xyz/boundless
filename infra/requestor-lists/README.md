# Requestor Lists Infrastructure

This Pulumi package deploys the Boundless requestor priority lists to S3, with optional CloudFront CDN and custom domain support.

## Prerequisites

- Node.js and npm
- Pulumi CLI
- AWS credentials configured
- (Optional) Route 53 hosted zone for custom domain

## Setup

1. Install dependencies:
   ```bash
   npm install
   ```

2. Initialize a Pulumi stack (e.g., dev, staging, prod):
   ```bash
   pulumi stack init dev
   ```

3. Configure the stack:
   ```bash
   # Optional: Custom bucket name
   pulumi config set BUCKET_NAME your-custom-bucket-name

   # Optional: Custom domain (enables CloudFront + ACM certificate)
   pulumi config set DOMAIN requestor-lists.yourdomain.com
   ```

## Deployment Options

### Option 1: Basic S3 Deployment (No Custom Domain)

Deploy without setting `DOMAIN`:
```bash
pulumi up
```

This will:
- Create an S3 bucket with public read access
- Upload the `boundless-recommended-priority-list.json` file
- Output the S3 HTTPS URL

**Outputs:**
- `bucketName`: The name of the S3 bucket
- `listUrl`: The HTTPS URL to access the JSON file

### Option 2: Custom Domain with CloudFront

Configure a custom domain:
```bash
pulumi config set DOMAIN requestor-lists.yourdomain.com
pulumi up
```

This will:
- Create an S3 bucket (private)
- Create an ACM certificate for your domain (in us-east-1)
- Create a CloudFront distribution with the certificate
- Upload the JSON file
- Output certificate validation records and CloudFront details

**Outputs:**
- `bucketName`: The name of the S3 bucket
- `listUrl`: The S3 HTTPS URL (backup)
- `domain`: Your custom domain
- `certificateArn`: ACM certificate ARN
- `certificateValidationRecords`: DNS records needed for validation
- `cloudfrontDomain`: CloudFront distribution domain (e.g., d123456.cloudfront.net)
- `cloudfrontDistributionId`: CloudFront distribution ID
- `customDomainUrl`: Your custom domain URL (e.g., https://requestor-lists.yourdomain.com/...)

## DNS Configuration (Custom Domain Only)

After running `pulumi up` with a custom domain, you need to configure DNS in Route 53:

### Step 1: Validate the ACM Certificate

1. Get the validation records from the output:
   ```bash
   pulumi stack output certificateValidationRecords
   ```

2. In Route 53, create a CNAME record with the name and value from the output

3. Wait for certificate validation (can take 5-30 minutes)

### Step 2: Point Your Domain to CloudFront

1. Get the CloudFront domain:
   ```bash
   pulumi stack output cloudfrontDomain
   ```

2. In Route 53, create an A record (ALIAS) for your domain:
   - **Name**: `requestor-lists.yourdomain.com` (or whatever you configured)
   - **Type**: A - IPv4 address
   - **Alias**: Yes
   - **Alias Target**: Select the CloudFront distribution

   Or create a CNAME record:
   - **Name**: `requestor-lists.yourdomain.com`
   - **Type**: CNAME
   - **Value**: The CloudFront domain from the output

3. Wait for DNS propagation (usually a few minutes)

## Usage in Broker

Add the list URL to your broker configuration:

```toml
[market]
priority_requestor_lists = [
  # If using custom domain:
  "https://requestor-lists.yourdomain.com/boundless-recommended-priority-list.json"

  # If using S3 directly:
  # "https://your-bucket.s3.amazonaws.com/boundless-recommended-priority-list.json"
]
```

## Updating the List

To update the priority list:

1. Edit `../../requestor-lists/boundless-recommended-priority-list.json`
2. Run `pulumi up` to redeploy

The list will be automatically refreshed by brokers according to their configured refresh interval (default: 1 hour).

If using CloudFront, you may want to invalidate the cache:
```bash
aws cloudfront create-invalidation \
  --distribution-id $(pulumi stack output cloudfrontDistributionId) \
  --paths "/*"
```
