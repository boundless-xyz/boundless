import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

interface SamplePipelineArgs {
  connection: aws.codestarconnections.Connection;
  artifactBucket: aws.s3.Bucket;
  role: aws.iam.Role;
}

const APP_NAME = "sample";

export class SamplePipeline extends pulumi.ComponentResource {
  constructor(name: string, args: SamplePipelineArgs, opts?: pulumi.ComponentResourceOptions) {
    super("boundless:pipelines:SamplePipeline", name, args, opts);

    const { connection, artifactBucket, role } = args;

    const buildSpec = `
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
          - cd infra/${APP_NAME}
          - echo "DEPLOYING"
          - echo "pulumi up --yes"
    `;

    const buildProject = new aws.codebuild.Project(
      `${APP_NAME}-build`,
      {
        buildTimeout: 5,
        description: `Build project for ${APP_NAME}`,
        serviceRole: role.arn,
        environment: {
          computeType: "BUILD_GENERAL1_SMALL",
          //image: "pulumi/pulumi:latest",
          image: "aws/codebuild/amazonlinux2-x86_64-standard:4.0",
          type: "LINUX_CONTAINER",
          privilegedMode: true,
          environmentVariables: [
            {
              name: "DEPLOYMENT_ROLE_ARN",
              type: "PLAINTEXT",
              value: "arn:aws:iam::245178712747:role/deploymentRole-13fd149"
            }
          ]
        },
        artifacts: { type: "CODEPIPELINE" },
        source: {
          type: "CODEPIPELINE",
          buildspec: buildSpec
        }
      },
      { dependsOn: [role] }
    );


    new aws.codepipeline.Pipeline(`${APP_NAME}-pipeline`, {
      artifactStores: [{
        type: "S3",
        location: artifactBucket.bucket
      }],
      stages: [
        {
          name: "Source",
          actions: [{
              name: "Source",
              category: "Source",
              owner: "AWS",
              provider: "CodeStarSourceConnection",
              version: "1",
              outputArtifacts: ["source_output"],
              configuration: {
                  ConnectionArn: connection.arn,
                  FullRepositoryId: "boundless-xyz/boundless",
                  BranchName: "willpote/init-deploy-aws",
              },
          }],
        },
        {
          name: "Build",
          actions: [
            {
              name: "DeployStaging",
              category: "Build",
              owner: "AWS",
              provider: "CodeBuild",
              version: "1",
              runOrder: 1,
              configuration: {
                ProjectName: buildProject.name
              },
              outputArtifacts: ["staging_output"],
              inputArtifacts: ["source_output"],
            }
          ]
        }
      ],
      name: `${APP_NAME}-pipeline`,
      roleArn: role.arn,
    });
  }




}
