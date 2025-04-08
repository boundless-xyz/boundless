import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export class Notifications extends pulumi.ComponentResource {
  public slackSNSTopic: aws.sns.Topic;

  constructor(
    name: string,
    args: {
      serviceAccountIds: string[];
      slackChannelId: pulumi.Output<string>;
      slackTeamId: pulumi.Output<string>;
    },
    opts?: pulumi.ComponentResourceOptions
  ) {
    super('pipelines:Notifications', name, args, opts);

    const { serviceAccountIds, slackChannelId: slackChannelIdOutput, slackTeamId: slackTeamIdOutput } = args;

    // Create an IAM Role for AWS Chatbot
    const chatbotRole = new aws.iam.Role('chatbotRole', {
      assumeRolePolicy: {
          Version: '2012-10-17',
          Statement: [
              {
                  Effect: 'Allow',
                  Principal: {
                      Service: 'chatbot.amazonaws.com',
                  },
                  Action: 'sts:AssumeRole',
              },
          ],
      },
      managedPolicyArns: [
        'arn:aws:iam::aws:policy/AmazonSNSReadOnlyAccess',
        'arn:aws:iam::aws:policy/CloudWatchReadOnlyAccess',
        'arn:aws:iam::aws:policy/AmazonQDeveloperAccess',
        'arn:aws:iam::aws:policy/AIOpsOperatorAccess',
      ],
    });

    // Create an SNS topic for the alerts
    this.slackSNSTopic = new aws.sns.Topic("boundless-alerts-topic", { name: "boundless-alerts-topic" });

    // Create a policy that allows the service accounts to publish to the SNS topic
    // https://repost.aws/knowledge-center/cloudwatch-cross-account-sns
    const snsTopicPolicy = this.slackSNSTopic.arn.apply(arn => aws.iam.getPolicyDocumentOutput({
      statements: [
        ...serviceAccountIds.map(serviceAccountId => ({
          actions: [
              "SNS:Publish",
          ],
          effect: "Allow",
          principals: [{
              type: "AWS",
              identifiers: ["*"], // Restricted by the condition below.
          }],
          resources: [arn],
          conditions: [{
            test: "ArnLike",
            variable: "aws:SourceArn",
            values: [`arn:aws:cloudwatch:us-west-2:${serviceAccountId}:alarm:*`],
          }],
          sid: `Grant publish to account ${serviceAccountId}`,
        })),
        {
          actions: ["SNS:Publish"],
          principals: [{
              type: "Service",
              identifiers: ["codestar-notifications.amazonaws.com"],
          }],
          resources: [arn],
          sid: "Grant publish to codestar for deployment notifications",
        },
      ],
    }));

    // Attach the policy to the SNS topic
    new aws.sns.TopicPolicy("service-accounts-publish-policy", {
        arn: this.slackSNSTopic.arn,
        policy: snsTopicPolicy.apply(snsTopicPolicy => snsTopicPolicy.json),
    });

    // Create a Slack channel configuration for the alerts
    let slackChannelConfiguration = pulumi.all([slackChannelIdOutput, slackTeamIdOutput])
      .apply(([slackChannelId, slackTeamId]) => new aws.chatbot.SlackChannelConfiguration("boundless-alerts", {
        configurationName: "boundless-alerts",
        iamRoleArn: chatbotRole.arn,
        slackChannelId: slackChannelId,
        slackTeamId: slackTeamId,
        snsTopicArns: [this.slackSNSTopic.arn],
        loggingLevel: "INFO",
      }));
    
  }
}
