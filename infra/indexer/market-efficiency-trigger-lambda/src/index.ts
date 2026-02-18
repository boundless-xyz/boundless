import { ECSClient, RunTaskCommand } from '@aws-sdk/client-ecs';
import { Handler } from 'aws-lambda';

export const handler: Handler = async () => {
  const required = ['CLUSTER_ARN', 'TASK_DEFINITION_ARN', 'SUBNET_IDS', 'SECURITY_GROUP_ID'];
  for (const key of required) {
    if (!process.env[key]) {
      throw new Error(`Missing required environment variable: ${key}`);
    }
  }

  const subnets = process.env.SUBNET_IDS!.split(',');
  const ecsClient = new ECSClient({ region: process.env.AWS_REGION || 'us-west-2' });

  const response = await ecsClient.send(
    new RunTaskCommand({
      cluster: process.env.CLUSTER_ARN,
      taskDefinition: process.env.TASK_DEFINITION_ARN,
      launchType: 'FARGATE',
      networkConfiguration: {
        awsvpcConfiguration: {
          subnets,
          securityGroups: [process.env.SECURITY_GROUP_ID!],
          assignPublicIp: 'DISABLED',
        },
      },
    })
  );

  const taskArn = response.tasks?.[0]?.taskArn;
  if (!taskArn) {
    throw new Error('No task ARN returned from ECS');
  }
  console.log('Market efficiency task started:', taskArn);
  return { taskArn };
};
