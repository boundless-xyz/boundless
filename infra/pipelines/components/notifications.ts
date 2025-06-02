import * as aws from '@pulumi/aws';
import * as pulumi from '@pulumi/pulumi';

export class Notifications extends pulumi.ComponentResource {
  public slackSNSTopic: aws.sns.Topic;
  public slackSNSTopicStaging: aws.sns.Topic;
  public pagerdutySNSTopic: aws.sns.Topic;

  constructor(
    name: string,
    args: {
      serviceAccountIds: string[];
      prodSlackChannelId: pulumi.Output<string>;
      stagingSlackChannelId: pulumi.Output<string>;
      slackTeamId: pulumi.Output<string>;
      pagerdutyIntegrationUrl: pulumi.Output<string>;
      ssoBaseUrl: string;
      runbookUrl: string;
      opsAccountId: string;
    },
    opts?: pulumi.ComponentResourceOptions
  ) {
    super('pipelines:Notifications', name, args, opts);

    const {
      serviceAccountIds,
      prodSlackChannelId: prodSlackChannelIdOutput,
      stagingSlackChannelId: stagingSlackChannelIdOutput,
      slackTeamId: slackTeamIdOutput,
      pagerdutyIntegrationUrl,
      ssoBaseUrl,
      runbookUrl
    } = args;

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
        'arn:aws:iam::aws:policy/AWSLambda_FullAccess',
      ],
    });

    // Create an IAM Role for AWS SNS to log delivery status to Cloudwatch
    // https://docs.aws.amazon.com/sns/latest/dg/topics-attrib-prereq.html
    const snsLoggingRole = new aws.iam.Role('snsLoggingRole', {
      assumeRolePolicy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Principal: {
              Service: 'sns.amazonaws.com',
            },
            Action: 'sts:AssumeRole',
          },
        ],
      },
      inlinePolicies: [
        {
          name: "snsLoggingPolicy",
          policy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [{
              Effect: "Allow",
              Action: [
                "logs:CreateLogGroup",
                "logs:CreateLogStream",
                "logs:PutLogEvents",
                "logs:PutMetricFilter",
                "logs:PutRetentionPolicy"
              ],
              Resource: "arn:aws:logs:*:*:*"
            }],
          }),
        },
      ],
    });

    const snsLoggingRoleArn = pulumi.interpolate`${snsLoggingRole.arn}`;

    // Create an SNS topic for the slack alerts
    this.slackSNSTopic = new aws.sns.Topic("boundless-alerts-topic", {
      name: "boundless-alerts-topic",
      applicationFailureFeedbackRoleArn: snsLoggingRoleArn,
      applicationSuccessFeedbackRoleArn: snsLoggingRoleArn,
      applicationSuccessFeedbackSampleRate: 100,
      httpFailureFeedbackRoleArn: snsLoggingRoleArn,
      httpSuccessFeedbackRoleArn: snsLoggingRoleArn,
      httpSuccessFeedbackSampleRate: 100,
      sqsFailureFeedbackRoleArn: snsLoggingRoleArn,
      sqsSuccessFeedbackRoleArn: snsLoggingRoleArn,
      sqsSuccessFeedbackSampleRate: 100,
    } as aws.sns.TopicArgs);

    // Create an SNS topic for the slack alerts
    this.slackSNSTopicStaging = new aws.sns.Topic("boundless-alerts-topic-staging", {
      name: "boundless-alerts-topic-staging",
      applicationFailureFeedbackRoleArn: snsLoggingRoleArn,
      applicationSuccessFeedbackRoleArn: snsLoggingRoleArn,
      applicationSuccessFeedbackSampleRate: 100,
      httpFailureFeedbackRoleArn: snsLoggingRoleArn,
      httpSuccessFeedbackRoleArn: snsLoggingRoleArn,
      httpSuccessFeedbackSampleRate: 100,
      sqsFailureFeedbackRoleArn: snsLoggingRoleArn,
      sqsSuccessFeedbackRoleArn: snsLoggingRoleArn,
      sqsSuccessFeedbackSampleRate: 100,
    } as aws.sns.TopicArgs);

    // Create a policy that allows the service accounts to publish to the SNS topic
    // https://repost.aws/knowledge-center/cloudwatch-cross-account-sns
    const slackSnsTopicPolicy = this.slackSNSTopic.arn.apply(arn => aws.iam.getPolicyDocumentOutput({
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
          sid: `Grant publish to account ${serviceAccountId}.`,
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

    const slackSnsTopicPolicyStaging = this.slackSNSTopicStaging.arn.apply(arn => aws.iam.getPolicyDocumentOutput({
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
          sid: `Grant publish to account ${serviceAccountId}.`,
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

    // Create an IAM Role for Log Fetcher Lambda
    const chatbotLogFetcherRole = new aws.iam.Role('chatbot-log-fetcher-role', {
      assumeRolePolicy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Principal: {
              Service: 'lambda.amazonaws.com',
            },
            Action: 'sts:AssumeRole',
          },
        ],
      },
      managedPolicyArns: [
        'arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole',
      ],
    });

    const chatbotLogFetcher = new aws.lambda.Function("chatbot-debugger", {
      handler: "index.handler",
      runtime: "nodejs20.x",
      role: chatbotLogFetcherRole.arn,
      code: new pulumi.asset.AssetArchive({
        '.': new pulumi.asset.FileArchive('./log-lambda/build'),
      }),
      environment: {
        variables: {
          SSO_BASE_URL: ssoBaseUrl,
          RUNBOOK_URL: runbookUrl,
        },
      },
    });

    // Attach the policy to the SNS topic
    slackSnsTopicPolicy.apply(slackSnsTopicPolicy => {
      new aws.sns.TopicPolicy("service-accounts-slack-publish-policy", {
        arn: this.slackSNSTopic.arn,
        policy: slackSnsTopicPolicy.json,
      }, {
        parent: this,
      });
      new aws.sns.TopicPolicy("service-accounts-slack-publish-policy-staging", {
        arn: this.slackSNSTopicStaging.arn,
        policy: slackSnsTopicPolicyStaging.json,
      }, {
        parent: this,
      });
    });

    // Create a dead letter queue for the Slack channel subscription.
    // NOTE: Pulumi does not support attaching dead letter queues to slack channel subscriptions,
    // so we manually attach this in the console.
    const sqsDeadLetterQueue = new aws.sqs.Queue("sqsDeadLetterQueue", {
      name: "sqsDeadLetterQueue",
    });
    new aws.sqs.QueuePolicy("sqsDeadLetterQueuePolicy", {
      queueUrl: sqsDeadLetterQueue.id,
      policy: {
        Version: "2012-10-17",
        Statement: [{
          Effect: "Allow",
          Principal: {
            Service: "sns.amazonaws.com",
          },
          Action: "sqs:SendMessage",
          Resource: sqsDeadLetterQueue.arn,
        }, {
          Effect: "Allow",
          Principal: {
            Service: "chatbot.amazonaws.com",
          },
          Action: "sqs:SendMessage",
          Resource: sqsDeadLetterQueue.arn,
        }, {
          Effect: "Allow",
          Principal: {
            AWS: args.opsAccountId,
          },
          Action: "sqs:GetQueueAttributes",
          Resource: sqsDeadLetterQueue.arn,
        }],
      },
    });

    // Create an IAM Role for AWS Chatbot
    const chatbotLogFetcherRole = new aws.iam.Role('chatbot-log-fetcher-role', {
      assumeRolePolicy: {
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Principal: {
              Service: 'lambda.amazonaws.com',
            },
            Action: 'sts:AssumeRole',
          },
        ],
      },
      managedPolicyArns: [
        'arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole',
      ],
    });

    const chatbotLogFetcher = new aws.lambda.Function("chatbot-debugger", {
      handler: "index.handler",
      runtime: "nodejs20.x",
      role: chatbotLogFetcherRole.arn,
      code: new pulumi.asset.AssetArchive({
        '.': new pulumi.asset.FileArchive('./log-lambda/build'),
      }),
      environment: {
        variables: {
          SSO_BASE_URL: ssoBaseUrl,
          RUNBOOK_URL: runbookUrl,
        },
      },
    });

    // Create a Slack channel configuration for the alerts
    let slackChannelConfigurations = pulumi.all([prodSlackChannelIdOutput, stagingSlackChannelIdOutput, slackTeamIdOutput])
      .apply(([prodSlackChannelId, stagingSlackChannelId, slackTeamId]) => {
        const prodSlackChannelConfiguration = new aws.chatbot.SlackChannelConfiguration("boundless-alerts", {
          configurationName: "boundless-alerts",
          iamRoleArn: chatbotRole.arn,
          slackChannelId: prodSlackChannelId,
          slackTeamId: slackTeamId,
          snsTopicArns: [this.slackSNSTopic.arn],
          loggingLevel: "INFO",
        });
        const stagingSlackChannelConfiguration = new aws.chatbot.SlackChannelConfiguration("boundless-alerts-staging", {
          configurationName: "boundless-alerts-staging",
          iamRoleArn: chatbotRole.arn,
          slackChannelId: stagingSlackChannelId,
          slackTeamId: slackTeamId,
          snsTopicArns: [this.slackSNSTopicStaging.arn],
          loggingLevel: "INFO",
        });
        return [prodSlackChannelConfiguration, stagingSlackChannelConfiguration];
      }
      );

    // Create an SNS topic for the pagerduty alerts
    this.pagerdutySNSTopic = new aws.sns.Topic("boundless-pagerduty-topic", {
      name: "boundless-pagerduty-topic",
      applicationFailureFeedbackRoleArn: snsLoggingRoleArn,
      applicationSuccessFeedbackRoleArn: snsLoggingRoleArn,
      applicationSuccessFeedbackSampleRate: 100,
      httpFailureFeedbackRoleArn: snsLoggingRoleArn,
      httpSuccessFeedbackRoleArn: snsLoggingRoleArn,
      httpSuccessFeedbackSampleRate: 100,
      sqsFailureFeedbackRoleArn: snsLoggingRoleArn,
      sqsSuccessFeedbackRoleArn: snsLoggingRoleArn,
      sqsSuccessFeedbackSampleRate: 100,
    } as aws.sns.TopicArgs);

    // Create a policy that allows the service accounts to publish to the SNS topic
    // https://repost.aws/knowledge-center/cloudwatch-cross-account-sns
    const pagerdutySnsTopicPolicy = this.pagerdutySNSTopic.arn.apply(arn => aws.iam.getPolicyDocumentOutput({
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
    new aws.sns.TopicPolicy("service-accounts-pagerduty-publish-policy", {
      arn: this.pagerdutySNSTopic.arn,
      policy: pagerdutySnsTopicPolicy.apply(pagerdutySnsTopicPolicy => pagerdutySnsTopicPolicy.json),
    });

    // Create an SNS subscription for the pagerduty alerts
    new aws.sns.TopicSubscription("boundless-pagerduty-subscription", {
      topic: this.pagerdutySNSTopic,
      protocol: "https",
      endpoint: pagerdutyIntegrationUrl,
      rawMessageDelivery: false,
    });
  }

};
