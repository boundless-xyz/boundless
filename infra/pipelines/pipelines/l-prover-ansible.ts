import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BasePipelineArgs } from "./base";

const APP_NAME = "prover-ansible";
const BUILD_TIMEOUT = 30;
const COMPUTE_TYPE = "BUILD_GENERAL1_MEDIUM";

export interface LProverAnsiblePipelineArgs extends BasePipelineArgs { }

/**
 * CodePipeline that deploys the prover via Ansible (monitoring, security, prover playbooks).
 * SSH key and inventory are read from AWS Secrets Manager so infra details are not in GitHub.
 *
 * Required secrets (must have a value via put-secret-value before first run, or build fails at DOWNLOAD_SOURCE):
 * - l-prover-ansible-ssh-key: SSH private key for Ansible (same as ANSIBLE_SSH_PRIVATE_KEY in GitHub).
 * - l-prover-ansible-inventory: Base64-encoded Ansible inventory. See ansible/INVENTORY.md for format and how to update.
 *
 * Pipeline stages:
 * 1. Source - fetch code from GitHub
 * 2. DeployStaging - deploy to staging environment
 * 3. DeployNightly - deploy to nightly (production:&nightly)
 * 4. DeployProduction - manual approval then deploy to production release (production:&release)
 */
export class LProverAnsiblePipeline extends pulumi.ComponentResource {
  public readonly pipelineName: pulumi.Output<string>;
  public readonly pipeline: aws.codepipeline.Pipeline;

  constructor(
    name: string,
    args: LProverAnsiblePipelineArgs,
    opts?: pulumi.ComponentResourceOptions
  ) {
    super("boundless:pipelines:l-prover-ansible", name, args, opts);

    const { connection, artifactBucket, role, slackAlertsTopicArn } = args;

    const sshKeySecret = new aws.secretsmanager.Secret(
      `${APP_NAME}-ssh-key`,
      { name: "l-prover-ansible-private-key" },
      { parent: this }
    );

    const inventorySecret = new aws.secretsmanager.Secret(
      `${APP_NAME}-inventory`,
      { name: "l-prover-ansible-inventory-file" },
      { parent: this }
    );

    new aws.iam.RolePolicy(
      `${APP_NAME}-secrets-policy`,
      {
        role: role.id,
        policy: pulumi.all([sshKeySecret.arn, inventorySecret.arn]).apply(
          ([sshArn, invArn]) =>
            JSON.stringify({
              Version: "2012-10-17",
              Statement: [
                {
                  Effect: "Allow",
                  Action: "secretsmanager:GetSecretValue",
                  Resource: [sshArn, invArn],
                },
              ],
            })
        ),
      },
      { parent: this }
    );

    const buildSpec = `version: 0.2
phases:
  install:
    commands:
      - apt-get update && apt-get install -y openssh-client python3-pip
      - pip install --break-system-packages ansible-core
      - pip uninstall -y paramiko || true
      - ansible-galaxy collection install community.postgresql
  pre_build:
    commands:
      - set -e
      - mkdir -p ~/.ssh
      - chmod 700 ~/.ssh
      - echo "$ANSIBLE_PRIVATE_KEY" > ~/.ssh/id_ed25519
      - chmod 600 ~/.ssh/id_ed25519
      - echo "$ANSIBLE_INVENTORY" | base64 -d > ansible/inventory.yml
  build:
    commands:
      - |
        set -e
        # Start ssh-agent and add key
        eval "$(ssh-agent -s)"
        ssh-add $HOME/.ssh/id_ed25519

        # Ansible environment
        export ANSIBLE_HOST_KEY_CHECKING=False
        export ANSIBLE_FORCE_COLOR=1
        export ANSIBLE_SSH_AGENT=auto
        export ANSIBLE_SSH_ARGS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null"

        cd ansible
        echo "Deploying to target: $TARGET"
        ansible-playbook -i inventory.yml monitoring.yml -D -v --limit "$TARGET"
        ansible-playbook -i inventory.yml security.yml -D -v --limit "$TARGET"
        ansible-playbook -i inventory.yml prover.yml -D -v --limit "$TARGET"
  post_build:
    commands:
      - rm -f ~/.ssh/id_ed25519
      - rm -f ansible/inventory.yml
artifacts:
  files: ['**/*']
`;

    const buildProject = new aws.codebuild.Project(
      `l-${APP_NAME}-build`,
      {
        name: `l-${APP_NAME}-build`,
        description: "Deploy prover via Ansible (monitoring, security, prover)",
        serviceRole: role.arn,
        buildTimeout: BUILD_TIMEOUT,
        environment: {
          computeType: COMPUTE_TYPE,
          image: "aws/codebuild/standard:7.0",
          type: "LINUX_CONTAINER",
          environmentVariables: [
            {
              name: "ANSIBLE_PRIVATE_KEY",
              type: "SECRETS_MANAGER",
              value: sshKeySecret.name,
            },
            {
              name: "ANSIBLE_INVENTORY",
              type: "SECRETS_MANAGER",
              value: inventorySecret.name,
            },
            {
              name: "TARGET",
              type: "PLAINTEXT",
              value: "staging",
            },
          ],
        },
        artifacts: { type: "CODEPIPELINE" },
        source: {
          type: "CODEPIPELINE",
          buildspec: buildSpec,
        },
        tags: {
          Name: `l-${APP_NAME}-deployment`,
          Component: `l-${APP_NAME}`,
        },
      },
      { parent: this, dependsOn: [role] }
    );

    const pipeline = new aws.codepipeline.Pipeline(
      `l-${APP_NAME}-pipeline`,
      {
        pipelineType: "V2",
        name: `l-${APP_NAME}-pipeline`,
        roleArn: role.arn,
        artifactStores: [
          {
            type: "S3",
            location: artifactBucket.bucket,
          },
        ],
        stages: [
          {
            name: "Source",
            actions: [
              {
                name: "Github",
                category: "Source",
                owner: "AWS",
                provider: "CodeStarSourceConnection",
                version: "1",
                outputArtifacts: ["source_output"],
                configuration: {
                  ConnectionArn: connection.arn,
                  FullRepositoryId: "boundless-xyz/boundless",
                  BranchName: "main",
                  OutputArtifactFormat: "CODEBUILD_CLONE_REF",
                },
              },
            ],
          },
          {
            name: "DeployStaging",
            actions: [
              {
                name: "AnsibleDeployStaging",
                category: "Build",
                owner: "AWS",
                provider: "CodeBuild",
                version: "1",
                runOrder: 1,
                inputArtifacts: ["source_output"],
                outputArtifacts: ["staging_output"],
                configuration: {
                  ProjectName: buildProject.name,
                  EnvironmentVariables: JSON.stringify([
                    {
                      name: "TARGET",
                      value: "staging",
                      type: "PLAINTEXT",
                    },
                  ]),
                },
              },
            ],
          },
          {
            name: "DeployNightly",
            actions: [
              {
                name: "AnsibleDeployProductionNightly",
                category: "Build",
                owner: "AWS",
                provider: "CodeBuild",
                version: "1",
                runOrder: 1,
                inputArtifacts: ["source_output"],
                outputArtifacts: ["nightly_output"],
                configuration: {
                  ProjectName: buildProject.name,
                  EnvironmentVariables: JSON.stringify([
                    {
                      name: "TARGET",
                      value: "production:&nightly",
                      type: "PLAINTEXT",
                    },
                  ]),
                },
              },
            ],
          },
          {
            name: "DeployProduction",
            actions: [
              {
                name: "ApproveDeployToProduction",
                category: "Approval",
                owner: "AWS",
                provider: "Manual",
                version: "1",
                runOrder: 1,
                configuration: {},
              },
              {
                name: "AnsibleDeployProduction",
                category: "Build",
                owner: "AWS",
                provider: "CodeBuild",
                version: "1",
                runOrder: 2,
                inputArtifacts: ["source_output"],
                outputArtifacts: ["production_output"],
                configuration: {
                  ProjectName: buildProject.name,
                  EnvironmentVariables: JSON.stringify([
                    {
                      name: "TARGET",
                      value: "production:&release",
                      type: "PLAINTEXT",
                    },
                  ]),
                },
              },
            ],
          },
        ],
        triggers: [
          {
            providerType: "CodeStarSourceConnection",
            gitConfiguration: {
              sourceActionName: "Github",
              pushes: [
                {
                  branches: {
                    includes: ["main"],
                  },
                },
              ],
            },
          },
        ],
        tags: {
          Name: `l-${APP_NAME}-pipeline`,
          Component: `l-${APP_NAME}`,
        },
      },
      { parent: this }
    );

    new aws.codestarnotifications.NotificationRule(
      `l-${APP_NAME}-pipeline-notifications`,
      {
        name: `l-${APP_NAME}-pipeline-notifications`,
        eventTypeIds: [
          "codepipeline-pipeline-pipeline-execution-failed",
          "codepipeline-pipeline-action-execution-failed",
          "codepipeline-pipeline-pipeline-execution-succeeded",
          "codepipeline-pipeline-manual-approval-needed",
        ],
        resource: pipeline.arn,
        detailType: "FULL",
        targets: [
          {
            address: slackAlertsTopicArn,
          },
        ],
        tags: {
          Name: `l-${APP_NAME}-pipeline-notifications`,
          Component: `l-${APP_NAME}`,
        },
      },
      { parent: this }
    );

    this.pipeline = pipeline;
    this.pipelineName = pipeline.name;
    this.registerOutputs({ pipelineName: pipeline.name });
  }
}
