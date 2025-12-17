import { ECSClient, RunTaskCommand } from '@aws-sdk/client-ecs';
import { Handler, Context } from 'aws-lambda';

interface BackfillEvent {
  mode?: 'statuses_and_aggregates' | 'aggregates';
  startBlock?: number;
  endBlock?: number;
  txFetchStrategy?: 'block-receipts' | 'tx-by-hash';
  // EventBridge scheduled event fields
  source?: string;
  'detail-type'?: string;
}

interface BackfillResponse {
  taskArn?: string;
  message: string;
  error?: string;
}

/**
 * Determine if this is an EventBridge scheduled event
 * EventBridge scheduled events have: { source: 'aws.events', 'detail-type': 'Scheduled Event', ... }
 */
function isScheduledEvent(event: any): boolean {
  return event && (event.source === 'aws.events' || event['detail-type'] === 'Scheduled Event');
}

export const handler: Handler<BackfillEvent, BackfillResponse> = async (
  event: BackfillEvent,
  context: Context
): Promise<BackfillResponse> => {
  console.log('Received backfill request', { event, context });

  // Validate required environment variables
  const requiredEnvVars = [
    'CLUSTER_ARN',
    'TASK_DEFINITION_ARN',
    'SUBNET_IDS',
    'SECURITY_GROUP_ID',
    'CONTAINER_NAME',
    'RPC_URL',
    'BOUNDLESS_ADDRESS',
    'CACHE_BUCKET',
  ];

  for (const envVar of requiredEnvVars) {
    if (!process.env[envVar]) {
      throw new Error(`Missing required environment variable: ${envVar}`);
    }
  }

  // Handle scheduled events vs manual invocations
  let mode: 'statuses_and_aggregates' | 'aggregates';
  let startBlock: number;

  if (isScheduledEvent(event)) {
    // Scheduled event - use defaults from environment
    mode = (process.env.SCHEDULED_BACKFILL_MODE as 'statuses_and_aggregates' | 'aggregates') || 'aggregates';

    // Use the same START_BLOCK as the regular indexer
    const startBlockEnv = process.env.START_BLOCK;
    if (!startBlockEnv) {
      return {
        message: 'Missing required environment variable',
        error: 'START_BLOCK is required for scheduled backfill',
      };
    }
    startBlock = parseInt(startBlockEnv, 10);
    if (isNaN(startBlock)) {
      return {
        message: 'Invalid START_BLOCK',
        error: `START_BLOCK must be a valid number, got: ${startBlockEnv}`,
      };
    }

    console.log(`Scheduled backfill: mode=${mode}, startBlock=${startBlock}`);
  } else {
    // Manual invocation - require explicit parameters
    if (!event.mode || event.startBlock === undefined) {
      return {
        message: 'Missing required parameters',
        error: 'mode and startBlock are required for manual invocations',
      };
    }
    mode = event.mode;
    startBlock = event.startBlock;
  }

  if (!['statuses_and_aggregates', 'aggregates'].includes(mode)) {
    return {
      message: 'Invalid mode',
      error: 'mode must be "statuses_and_aggregates" or "aggregates"',
    };
  }

  // Build command
  const command: string[] = [
    '--mode', mode,
    '--rpc-url', process.env.RPC_URL!,
    '--boundless-market-address', process.env.BOUNDLESS_ADDRESS!,
    '--start-block', startBlock.toString(),
    '--log-json',
    '--cache-uri', `s3://${process.env.CACHE_BUCKET}`,
    '--tx-fetch-strategy', event.txFetchStrategy || 'tx-by-hash',
  ];

  if (process.env.LOGS_RPC_URL) {
    command.push('--logs-rpc-url', process.env.LOGS_RPC_URL);
  }

  if (event.endBlock) {
    command.push('--end-block', event.endBlock.toString());
  }

  console.log('Running task with command:', command);

  // Parse subnets (comma-separated string to array)
  const subnets = process.env.SUBNET_IDS!.split(',');

  // Run ECS task
  const ecsClient = new ECSClient({ region: process.env.AWS_REGION || 'us-west-2' });

  try {
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
        overrides: {
          containerOverrides: [
            {
              name: process.env.CONTAINER_NAME,
              command,
            },
          ],
        },
      })
    );

    const taskArn = response.tasks?.[0]?.taskArn;

    if (!taskArn) {
      return {
        message: 'Failed to start task',
        error: 'No task ARN returned from ECS',
      };
    }

    console.log('Task started successfully:', taskArn);

    return {
      taskArn,
      message: 'Backfill task started successfully',
    };
  } catch (error) {
    console.error('Error starting ECS task:', error);
    return {
      message: 'Error starting task',
      error: error instanceof Error ? error.message : String(error),
    };
  }
};

