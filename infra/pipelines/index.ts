import { PulumiStateBucket } from "./components/pulumiState";
import { PulumiSecrets } from "./components/pulumiSecrets";
import { Notifications } from "./components/notifications";
import { LDistributorPipeline } from "./pipelines/l-distributor";
import { LIndexerPipeline } from "./pipelines/l-indexer";
import { LOrderGeneratorPipeline } from "./pipelines/l-order-generator";
import { LOrderStreamPipeline } from "./pipelines/l-order-stream";
import { LProverAnsiblePipeline } from "./pipelines/l-prover-ansible";
import { LRequestorListsPipeline } from "./pipelines/l-requestor-lists";
import { LSlasherPipeline } from "./pipelines/l-slasher";
import { CodePipelineSharedResources } from "./components/codePipelineResources";
import * as aws from "@pulumi/aws";
import {
  BOUNDLESS_DEV_ADMIN_ROLE_ARN,
  BOUNDLESS_OPS_ACCOUNT_ID,
  BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
  BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
  BOUNDLESS_STAGING_ADMIN_ROLE_ARN,
  BOUNDLESS_PROD_ADMIN_ROLE_ARN,
  BOUNDLESS_STAGING_ACCOUNT_ID,
  BOUNDLESS_PROD_ACCOUNT_ID,
  BOUNDLESS_DEV_ACCOUNT_ID
} from "./accountConstants";
import * as pulumi from '@pulumi/pulumi';

// Shared pipeline resources (role, artifact bucket) must exist before PulumiStateBucket so the
// pipeline role can be granted access to the state bucket and KMS key (for CodeBuild pulumi login).
const codePipelineSharedResources = new CodePipelineSharedResources("codePipelineShared", {
  accountId: BOUNDLESS_OPS_ACCOUNT_ID,
  serviceAccountDeploymentRoleArns: [
    BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
    BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
  ],
});

// Defines the S3 bucket used for storing the Pulumi state backend for staging and prod accounts.
// The ops pipeline role is included so CodeBuild can run `pulumi login` / `pulumi destroy` before assuming cross-account roles.
const pulumiStateBucket = new PulumiStateBucket("pulumiStateBucket", {
  accountId: BOUNDLESS_OPS_ACCOUNT_ID,
  readOnlyStateBucketArns: [
    BOUNDLESS_DEV_ADMIN_ROLE_ARN,
  ],
  readWriteStateBucketArns: [
    BOUNDLESS_STAGING_ADMIN_ROLE_ARN,
    BOUNDLESS_PROD_ADMIN_ROLE_ARN,
    BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
    BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
    codePipelineSharedResources.role.arn,
  ],
});

// Defines the KMS key used to encrypt and decrypt secrets.
// Currently, developers logged in as Admin in the Boundless Dev account can encrypt and decrypt secrets.
// TODO: Only deployment roles should be allowed to decrypt secrets.
// Staging and prod deployment roles plus the ops pipeline role (for CodeBuild) can decrypt secrets.
const pulumiSecrets = new PulumiSecrets("pulumiSecrets", {
  accountId: BOUNDLESS_OPS_ACCOUNT_ID,
  encryptKmsKeyArns: [
    BOUNDLESS_DEV_ADMIN_ROLE_ARN
  ],
  decryptKmsKeyArns: [
    BOUNDLESS_DEV_ADMIN_ROLE_ARN,
    BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN,
    BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN,
    codePipelineSharedResources.role.arn,
  ],
});

// Defines the connection to the "AWS Connector for Github" app on Github.
// Note that the initial setup for the app requires a manual step that must be done in the console. If this
// resource is ever deleted, this step will need to be repeated. See:
// https://docs.aws.amazon.com/codepipeline/latest/userguide/connections-github.html
const githubConnection = new aws.codestarconnections.Connection("boundlessGithubConnection", {
  name: "boundlessGithubConnection",
  providerType: "GitHub",
});

const config = new pulumi.Config();
const boundlessAlertsSlackId = config.requireSecret("BOUNDLESS_ALERTS_SLACK_ID");
const boundlessAlertsStagingSlackId = config.requireSecret("BOUNDLESS_ALERTS_STAGING_SLACK_ID");
const boundlessAlertsLaunchSlackId = config.requireSecret("BOUNDLESS_ALERTS_LAUNCH_SLACK_ID");
const boundlessAlertsStagingLaunchSlackId = config.requireSecret("BOUNDLESS_ALERTS_STAGING_LAUNCH_SLACK_ID");
const workspaceSlackId = config.requireSecret("WORKSPACE_SLACK_ID");
const pagerdutyIntegrationUrl = config.requireSecret("PAGERDUTY_INTEGRATION_URL");
const ssoBaseUrl = config.require("SSO_BASE_URL");
const runbookUrl = config.require("RUNBOOK_URL");

const notifications = new Notifications("notifications", {
  opsAccountId: BOUNDLESS_OPS_ACCOUNT_ID,
  serviceAccountIds: [
    BOUNDLESS_OPS_ACCOUNT_ID,
    BOUNDLESS_STAGING_ACCOUNT_ID,
    BOUNDLESS_PROD_ACCOUNT_ID,
  ],
  prodSlackChannelId: boundlessAlertsSlackId,
  stagingSlackChannelId: boundlessAlertsStagingSlackId,
  prodLaunchSlackChannelId: boundlessAlertsLaunchSlackId,
  stagingLaunchSlackChannelId: boundlessAlertsStagingLaunchSlackId,
  slackTeamId: workspaceSlackId,
  pagerdutyIntegrationUrl,
  ssoBaseUrl,
  runbookUrl,
});

// The Docker and GH tokens are used to avoid rate limiting issues when building in the pipelines.
const githubToken = config.requireSecret("GITHUB_TOKEN");
const dockerUsername = config.require("DOCKER_USER");
const dockerToken = config.requireSecret("DOCKER_PAT");

// Launch pipelines
const lDistributorPipeline = new LDistributorPipeline("lDistributorPipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
  githubToken,
  dockerUsername,
  dockerToken,
  slackAlertsTopicArn: notifications.slackSNSTopicLaunch.arn,
})

const lOrderStreamPipeline = new LOrderStreamPipeline("lOrderStreamPipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
  githubToken,
  dockerUsername,
  dockerToken,
  slackAlertsTopicArn: notifications.slackSNSTopicLaunch.arn,
})

const lOrderGeneratorPipeline = new LOrderGeneratorPipeline("lOrderGeneratorPipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
  githubToken,
  dockerUsername,
  dockerToken,
  slackAlertsTopicArn: notifications.slackSNSTopicLaunch.arn,
})

const lIndexerPipeline = new LIndexerPipeline("lIndexerPipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
  githubToken,
  dockerUsername,
  dockerToken,
  slackAlertsTopicArn: notifications.slackSNSTopicLaunch.arn,
})

const lSlasherPipeline = new LSlasherPipeline("lSlasherPipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
  githubToken,
  dockerUsername,
  dockerToken,
  slackAlertsTopicArn: notifications.slackSNSTopicLaunch.arn,
})

const lRequestorListsPipeline = new LRequestorListsPipeline("lRequestorListsPipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
  githubToken,
  dockerUsername,
  dockerToken,
  slackAlertsTopicArn: notifications.slackSNSTopicLaunch.arn,
})

// Prover Ansible deployment (SSH + inventory in Secrets Manager; no infra in GitHub)
const lProverAnsiblePipeline = new LProverAnsiblePipeline("lProverAnsiblePipeline", {
  connection: githubConnection,
  artifactBucket: codePipelineSharedResources.artifactBucket,
  role: codePipelineSharedResources.role,
  githubToken,
  dockerUsername,
  dockerToken,
  slackAlertsTopicArn: notifications.slackSNSTopicLaunch.arn,
})

export const bucketName = pulumiStateBucket.bucket.id;
export const kmsKeyArn = pulumiSecrets.kmsKey.arn;
export const boundlessAlertsBetaTopicArn = notifications.slackSNSTopic.arn;
export const boundlessPagerdutyBetaTopicArn = notifications.pagerdutySNSTopic.arn;
export const boundlessAlertsTopicArnLaunch = notifications.slackSNSTopicLaunch.arn;
export const boundlessAlertsTopicArnStagingLaunch = notifications.slackSNSTopicStagingLaunch.arn;
export const lProverAnsiblePipelineName = lProverAnsiblePipeline.pipelineName;
