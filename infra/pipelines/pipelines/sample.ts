import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN } from "../accountConstants";

interface SamplePipelineArgs {
  connection: aws.codestarconnections.Connection;
  artifactBucket: aws.s3.Bucket;
  role: aws.iam.Role;
}

// The name of the app that we are deploying. Must match the name of the directory in the infra directory.
const APP_NAME = "sample";
// The branch that we should deploy from on push.
const BRANCH_NAME = "main";
// The buildspec for the CodeBuild project that deploys our Pulumi stacks to the staging and prod accounts.
// Note in pre-build we assume the deployment role for the given account before running pulumi commands, so
// that we deploy to the target account.
const BUILD_SPEC = `
    version: 0.2
    
    phases:
      pre_build:
        commands:
          - echo Assuming role $DEPLOYMENT_ROLE_ARN
          - ASSUMED_ROLE=$(aws sts assume-role --role-arn $DEPLOYMENT_ROLE_ARN --role-session-name Deployment --output text | tail -1)
          - export AWS_ACCESS_KEY_ID=$(echo $ASSUMED_ROLE | awk '{print $2}')
          - export AWS_SECRET_ACCESS_KEY=$(echo $ASSUMED_ROLE | awk '{print $4}')
          - export AWS_SESSION_TOKEN=$(echo $ASSUMED_ROLE | awk '{print $5}')
          - curl -fsSL https://get.pulumi.com/ | sh
          - export PATH=$PATH:$HOME/.pulumi/bin
          - pulumi login --non-interactive "s3://boundless-pulumi-state?region=us-west-2&awssdk=v2"
      build:
        commands:
          - ls -lt
          - cd infra/$APP_NAME
          - pulumi install
          - echo "DEPLOYING stack $STACK_NAME"
          - pulumi stack select $STACK_NAME
          - pulumi up --yes
    `;

// A sample deployment pipeline that deploys to the staging account, then requires a manual approval before
// deploying to prod.
export class SamplePipeline extends pulumi.ComponentResource {
  constructor(name: string, args: SamplePipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super("boundless:pipelines:SamplePipeline", name, args, opts);

    const { connection, artifactBucket, role } = args;

    const stagingDeployment = new aws.codebuild.Project(
      `${APP_NAME}-staging-build`,
      this.codeBuildProjectArgs(APP_NAME, "staging", role, BOUNDLESS_STAGING_DEPLOYMENT_ROLE_ARN),
      { dependsOn: [role] }
    );

    const prodDeployment = new aws.codebuild.Project(
      `${APP_NAME}-prod-build`,
      this.codeBuildProjectArgs(APP_NAME, "prod", role, BOUNDLESS_PROD_DEPLOYMENT_ROLE_ARN),
      { dependsOn: [role] }
    );

    new aws.codepipeline.Pipeline(`${APP_NAME}-pipeline`, {
      pipelineType: "V2",
      artifactStores: [{
        type: "S3",
        location: artifactBucket.bucket
      }],
      stages: [
        {
          name: "Github",
          actions: [{
              name: "Github",
              category: "Source",
              owner: "AWS",
              provider: "CodeStarSourceConnection",
              version: "1",
              outputArtifacts: ["source_output"],
              configuration: {
                  ConnectionArn: connection.arn,
                  FullRepositoryId: "boundless-xyz/boundless",
                  BranchName: BRANCH_NAME,
              },
          }],
        },
        {
          name: "DeployStaging",
          actions: [
            {
              name: "DeployStaging",
              category: "Build",
              owner: "AWS",
              provider: "CodeBuild",
              version: "1",
              runOrder: 1,
              configuration: {
                ProjectName: stagingDeployment.name
              },
              outputArtifacts: ["staging_output"],
              inputArtifacts: ["source_output"],
            }
          ]
        },
        {
          name: "DeployProduction",
          actions: [
            { name: "ApproveDeployToProduction", 
              category: "Approval", 
              owner: "AWS", 
              provider: "Manual", 
              version: "1", 
              runOrder: 1,
              configuration: {}
            },
            {
              name: "DeployProduction",
              category: "Build",
              owner: "AWS",
              provider: "CodeBuild",
              version: "1",
              runOrder: 2,
              configuration: {
                ProjectName: prodDeployment.name
              },
              outputArtifacts: ["production_output"],
              inputArtifacts: ["source_output"],
            }
          ]
        }
      ],
      triggers: [
        {
          providerType: "CodeStarSourceConnection",
          gitConfiguration: {
            sourceActionName: "Github",
            pushes: [
              {
                branches: {
                  includes: [BRANCH_NAME],
                },
              },
            ],
          },
        },
      ],
      name: `${APP_NAME}-pipeline`,
      roleArn: role.arn,
    });
  }

  private codeBuildProjectArgs(appName: string, stackName: string, role: aws.iam.Role, serviceAccountRoleArn: string): aws.codebuild.ProjectArgs {
    return {
      buildTimeout: 5,
      description: `Deployment for ${APP_NAME}`,
      serviceRole: role.arn,
      environment: {
        computeType: "BUILD_GENERAL1_SMALL",
        image: "aws/codebuild/amazonlinux2-x86_64-standard:4.0",
        type: "LINUX_CONTAINER",
        privilegedMode: true,
        environmentVariables: [
          {
            name: "DEPLOYMENT_ROLE_ARN",
            type: "PLAINTEXT",
            value: serviceAccountRoleArn
          },
          {
            name: "STACK_NAME",
            type: "PLAINTEXT",
            value: stackName
          },
          {
            name: "APP_NAME",
            type: "PLAINTEXT",
            value: appName
          }
        ]
      },
      artifacts: { type: "CODEPIPELINE" },
      source: {
        type: "CODEPIPELINE",
        buildspec: BUILD_SPEC
      }
    }
  }
}
