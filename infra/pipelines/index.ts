import { PulumiStateBucket } from "./components/pulumiState";
import { PulumiSecrets } from "./components/pulumiSecrets";
import { SamplePipeline } from "./pipelines/sample";
import { CodePipelineSharedResources } from "./components/codePipelineResources";
import * as aws from "@pulumi/aws";
import { 
  BOUNDLESS_DEV_ADMIN_ROLE_ARN, 
  BOUNDLESS_OPS_ACCOUNT_ID, 
  BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN, 
  BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN 
} from "./accountConstants";

// Defines the S3 bucket used for storing the Pulumi state backend for staging and prod accounts.
const pulumiStateBucket = new PulumiStateBucket("pulumiStateBucket", {
  accountId: BOUNDLESS_OPS_ACCOUNT_ID,
  readOnlyStateBucketArns: [
    BOUNDLESS_DEV_ADMIN_ROLE_ARN,
  ],
  readWriteStateBucketArns: [
    BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
    BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
  ],
});

// Defines the KMS key used to encrypt and decrypt secrets. We allow any developer logged in as Admin in
// Boundless dev account to encrypt secrets, but not decrypt them. Staging and prod deployement roles
// are the only accounts allowed to decrypt secrets.
const pulumiSecrets = new PulumiSecrets("pulumiSecrets", {
  accountId: BOUNDLESS_OPS_ACCOUNT_ID,
  encryptKmsKeyArns: [
    BOUNDLESS_DEV_ADMIN_ROLE_ARN
  ],
  decryptKmsKeyArns: [
    BOUNDLESS_DEV_ADMIN_ROLE_ARN,
    BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
    BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
  ],
});

// Defines the connection to the "AWS Connector for Github" app on Github.
// Note that this initial setup is a manual step that must be done in the console. See:
// https://docs.aws.amazon.com/codepipeline/latest/userguide/connections-github.html
const githubConnection = new aws.codestarconnections.Connection("boundlessGithubConnection", {
  name: "boundlessGithubConnection",
  providerType: "GitHub",
});

// Resouces that are shared between all deployment pipelines like IAM roles, S3 artifact buckets, etc.
const codePipelineSharedResources = new CodePipelineSharedResources("codePipelineShared", {
  accountId: BOUNDLESS_OPS_ACCOUNT_ID,
  serviceAccountDeploymentRoleArns: [
    BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
    BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
  ],
});

// Deploy the "sample" app.
const samplePipeline = new SamplePipeline("samplePipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
});

export const bucketName = pulumiStateBucket.bucket.id;
export const kmsKeyArn = pulumiSecrets.kmsKey.arn;