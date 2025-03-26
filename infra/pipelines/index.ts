import { PulumiStateBucket } from "./components/pulumiState";
import { PulumiSecrets } from "./components/pulumiSecrets";
import { SamplePipeline } from "./components/pipelines/sample";
import { CodePipelineSharedResources } from "./components/codePipelineResources";

import * as aws from "@pulumi/aws";
const boundlessOpsAccountId = "968153779208"; 
const boundlessDevAccountId = "751442549745";
const boundlessStagingAccountId = "245178712747";
const boundlessStagingDeploymentRoleArn = "arn:aws:iam::245178712747:role/deploymentRole-13fd149";
const boundlessProdAccountId = "632745187633";
const boundlessProdDeploymentRoleArn = "arn:aws:iam::632745187633:role/deploymentRole-09bd21a";

const boundlessDevAdminRoleArn = "arn:aws:iam::751442549745:role/aws-reserved/sso.amazonaws.com/us-east-2/AWSReservedSSO_AWSAdministratorAccess_05b42ccedab0fe1d";
const boundlessStagingAdminRoleArn = "arn:aws:iam::245178712747:role/aws-reserved/sso.amazonaws.com/us-east-2/AWSReservedSSO_AWSAdministratorAccess_9cd40e36ef4d960f";
const boundlessProdAdminRoleArn = "arn:aws:iam::632745187633:role/aws-reserved/sso.amazonaws.com/us-east-2/AWSReservedSSO_AWSAdministratorAccess_b887a89d07489007";

// Defines the S3 bucket used for storing the Pulumi state backend for staging and prod accounts.
const pulumiStateBucket = new PulumiStateBucket("pulumiStateBucket", {
  accountId: boundlessOpsAccountId,
  readWriteStateBucketArns: [
    boundlessStagingDeploymentRoleArn,
    boundlessProdDeploymentRoleArn,
  ],
});

// Defines the KMS key used to encrypt and decrypt secrets. We allow any developer logged in as Admin in
// Boundless dev account to encrypt secrets, but not decrypt them. Staging and prod deployement roles
// are the only accounts allowed to decrypt secrets.
const pulumiSecrets = new PulumiSecrets("pulumiSecrets", {
  accountId: boundlessOpsAccountId,
  encryptKmsKeyArns: [
    boundlessDevAdminRoleArn
  ],
  decryptKmsKeyArns: [
    boundlessStagingDeploymentRoleArn,
    boundlessProdDeploymentRoleArn,
  ],
});

const githubConnection = new aws.codestarconnections.Connection("boundlessGithubConnection", {
  name: "boundlessGithubConnection",
  providerType: "GitHub",
});

const codePipelineSharedResources = new CodePipelineSharedResources("codePipelineShared", {
  accountId: boundlessOpsAccountId,
  serviceAccountDeploymentRoleArns: [
    boundlessStagingDeploymentRoleArn,
    boundlessProdDeploymentRoleArn,
  ],
});

const samplePipeline = new SamplePipeline("samplePipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
});

export const bucketName = pulumiStateBucket.bucket.id;
export const kmsKeyArn = pulumiSecrets.kmsKey.arn;