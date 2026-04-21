import * as aws from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";

const PRODUCTION_STAGE_NAME = "DeployProduction";

export function createLaunchPipelineNotificationRule(
  name: string,
  pipelineArn: pulumi.Input<string>,
  eventTypeIds: string[],
  slackAlertsTopicArn: pulumi.Input<string>,
  tags: Record<string, pulumi.Input<string>>,
  opts?: pulumi.CustomResourceOptions
): aws.codestarnotifications.NotificationRule {
  return new aws.codestarnotifications.NotificationRule(
    name,
    {
      name,
      eventTypeIds,
      resource: pipelineArn,
      detailType: "FULL",
      targets: [
        {
          address: slackAlertsTopicArn,
        },
      ],
      tags,
    },
    opts
  );
}

export function createProdStageFailureAlert(
  name: string,
  pipelineArn: pulumi.Input<string>,
  slackAlertsTopicArn: pulumi.Input<string>,
  tags: Record<string, pulumi.Input<string>>,
  opts?: pulumi.ComponentResourceOptions
): {
  rule: aws.cloudwatch.EventRule;
  target: aws.cloudwatch.EventTarget;
} {
  const eventPattern = pulumi.output(pipelineArn).apply((arn) =>
    JSON.stringify({
      source: ["aws.codepipeline"],
      "detail-type": ["CodePipeline Stage Execution State Change"],
      resources: [arn],
      detail: {
        state: ["FAILED"],
        stage: [PRODUCTION_STAGE_NAME],
      },
    })
  );

  const rule = new aws.cloudwatch.EventRule(
    name,
    {
      name,
      eventPattern,
      tags,
    },
    opts
  );

  const target = new aws.cloudwatch.EventTarget(
    `${name}-target`,
    {
      rule: rule.name,
      arn: slackAlertsTopicArn,
    },
    opts
  );

  return { rule, target };
}
