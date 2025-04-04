import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export class Notifications extends pulumi.ComponentResource {
  public snsTopic: aws.sns.Topic;

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
        'arn:aws:iam::aws:policy/AmazonSNSFullAccess',
        'arn:aws:iam::aws:policy/CloudWatchReadOnlyAccess',
      ],
    });

    // Create an SNS topic for the alerts
    this.snsTopic = new aws.sns.Topic("boundless-alerts-topic", { name: "boundless-alerts-topic" });

    // Create a policy that allows the service accounts to publish to the SNS topic
    const snsTopicPolicy = this.snsTopic.arn.apply(arn => aws.iam.getPolicyDocumentOutput({
      statements: [{
          actions: [
              "SNS:Publish",
          ],
          effect: "Allow",
          principals: [{
              type: "AWS",
              identifiers: serviceAccountIds,
          }],
          resources: [arn],
          sid: "Grant publish to ops account andservice accounts",
      },
      {
          actions: ["SNS:Publish"],
          principals: [{
              type: "Service",
              identifiers: ["codestar-notifications.amazonaws.com"],
          }],
          resources: [arn],
          sid: "Grant publish to codestar for deployment notifications",
      }],
    }));

    // Attach the policy to the SNS topic
    new aws.sns.TopicPolicy("service-accounts-publish-policy", {
        arn: this.snsTopic.arn,
        policy: snsTopicPolicy.apply(snsTopicPolicy => snsTopicPolicy.json),
    });

    // Create a Slack channel configuration for the alerts
    let slackChannelConfiguration = pulumi.all([slackChannelIdOutput, slackTeamIdOutput])
      .apply(([slackChannelId, slackTeamId]) => new aws.chatbot.SlackChannelConfiguration("boundless-alerts", {
        configurationName: "boundless-alerts",
        iamRoleArn: chatbotRole.arn,
        slackChannelId: slackChannelId,
        slackTeamId: slackTeamId,
        snsTopicArns: [this.snsTopic.arn],
        loggingLevel: "INFO",
      }));
    
  }
}
