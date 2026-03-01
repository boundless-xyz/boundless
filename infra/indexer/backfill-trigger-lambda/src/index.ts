import { ECSClient, RunTaskCommand } from '@aws-sdk/client-ecs';
import { Handler, Context } from 'aws-lambda';

interface BackfillEvent {
  mode?: 'statuses_and_aggregates' | 'aggregates' | 'chain_data';
  startBlock?: number;
  endBlock?: number;
  txFetchStrategy?: 'block-receipts' | 'tx-by-hash';
  // Number of blocks to look back from current block (alternative to startBlock)
  lookbackBlocks?: number;
  // Delay in milliseconds between batches during chain data backfill
  chainDataBatchDelayMs?: number;
  // Number of blocks to process in each batch
  batchSize?: number;
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
  let mode: 'statuses_and_aggregates' | 'aggregates' | 'chain_data';
  let startBlock: number | undefined;
  let lookbackBlocks: number | undefined;
  let endBlock: number | undefined;
  let chainDataBatchDelayMs: number | undefined;
  let batchSize: number | undefined;

  if (isScheduledEvent(event)) {
    // Scheduled event - use all config from environment variables
    mode = (process.env.SCHEDULED_BACKFILL_MODE as 'statuses_and_aggregates' | 'aggregates' | 'chain_data') || 'aggregates';

    if (process.env.LOOKBACK_BLOCKS) {
      lookbackBlocks = parseInt(process.env.LOOKBACK_BLOCKS, 10);
      if (isNaN(lookbackBlocks) || lookbackBlocks <= 0) {
        return {
          message: 'Invalid LOOKBACK_BLOCKS',
          error: `LOOKBACK_BLOCKS must be a positive number, got: ${process.env.LOOKBACK_BLOCKS}`,
        };
      }
    } else if (process.env.START_BLOCK) {
      startBlock = parseInt(process.env.START_BLOCK, 10);
      if (isNaN(startBlock)) {
        return {
          message: 'Invalid START_BLOCK',
          error: `START_BLOCK must be a valid number, got: ${process.env.START_BLOCK}`,
        };
      }
    } else {
      return {
        message: 'Missing required environment variable',
        error: 'START_BLOCK or LOOKBACK_BLOCKS is required for scheduled backfill',
      };
    }

    // endBlock is not typically set for scheduled events (defaults to last indexed block)
    if (process.env.CHAIN_DATA_BATCH_DELAY_MS) {
      chainDataBatchDelayMs = parseInt(process.env.CHAIN_DATA_BATCH_DELAY_MS, 10);
      if (isNaN(chainDataBatchDelayMs) || chainDataBatchDelayMs < 0) {
        return {
          message: 'Invalid CHAIN_DATA_BATCH_DELAY_MS',
          error: `CHAIN_DATA_BATCH_DELAY_MS must be a non-negative number, got: ${process.env.CHAIN_DATA_BATCH_DELAY_MS}`,
        };
      }
    }
    if (process.env.BACKFILL_BATCH_SIZE) {
      batchSize = parseInt(process.env.BACKFILL_BATCH_SIZE, 10);
      if (isNaN(batchSize) || batchSize <= 0) {
        return {
          message: 'Invalid BACKFILL_BATCH_SIZE',
          error: `BACKFILL_BATCH_SIZE must be a positive number, got: ${process.env.BACKFILL_BATCH_SIZE}`,
        };
      }
    }
    console.log(`Scheduled backfill: mode=${mode}, startBlock=${startBlock}, lookbackBlocks=${lookbackBlocks}, chainDataBatchDelayMs=${chainDataBatchDelayMs}, batchSize=${batchSize}`);
  } else {
    // Manual invocation - use all config from event payload
    if (!event.mode) {
      return {
        message: 'Missing required parameters',
        error: 'mode is required for manual invocations',
      };
    }
    mode = event.mode;
    endBlock = event.endBlock;

    if (event.lookbackBlocks !== undefined) {
      lookbackBlocks = event.lookbackBlocks;
    } else if (event.startBlock !== undefined) {
      startBlock = event.startBlock;
    } else {
      return {
        message: 'Missing required parameters',
        error: 'startBlock or lookbackBlocks is required for manual invocations',
      };
    }

    if (event.chainDataBatchDelayMs !== undefined) {
      chainDataBatchDelayMs = event.chainDataBatchDelayMs;
      if (isNaN(chainDataBatchDelayMs) || chainDataBatchDelayMs < 0) {
        return {
          message: 'Invalid chainDataBatchDelayMs',
          error: `chainDataBatchDelayMs must be a non-negative number, got: ${chainDataBatchDelayMs}`,
        };
      }
    }
    if (event.batchSize !== undefined) {
      batchSize = event.batchSize;
      if (isNaN(batchSize) || batchSize <= 0) {
        return {
          message: 'Invalid batchSize',
          error: `batchSize must be a positive number, got: ${batchSize}`,
        };
      }
    }
    console.log(`Manual backfill: mode=${mode}, startBlock=${startBlock}, lookbackBlocks=${lookbackBlocks}, endBlock=${endBlock}, chainDataBatchDelayMs=${chainDataBatchDelayMs}, batchSize=${batchSize}`);
  }

  if (!['statuses_and_aggregates', 'aggregates', 'chain_data'].includes(mode)) {
    return {
      message: 'Invalid mode',
      error: 'mode must be "statuses_and_aggregates", "aggregates", or "chain_data"',
    };
  }

  // Build command - backfill image has ENTRYPOINT set to the binary
  const command: string[] = [
    '--mode', mode,
    '--rpc-url', process.env.RPC_URL!,
    '--boundless-market-address', process.env.BOUNDLESS_ADDRESS!,
    '--log-json',
    '--cache-uri', `s3://${process.env.CACHE_BUCKET}`,
    '--tx-fetch-strategy', event.txFetchStrategy || 'tx-by-hash',
  ];

  // Add either --start-block or --lookback-blocks
  if (lookbackBlocks !== undefined) {
    command.push('--lookback-blocks', lookbackBlocks.toString());
  } else if (startBlock !== undefined) {
    command.push('--start-block', startBlock.toString());
  }

  if (process.env.LOGS_RPC_URL) {
    command.push('--logs-rpc-url', process.env.LOGS_RPC_URL);
  }

  if (endBlock !== undefined) {
    command.push('--end-block', endBlock.toString());
  }

  if (chainDataBatchDelayMs !== undefined && chainDataBatchDelayMs > 0) {
    command.push('--chain-data-batch-delay-ms', chainDataBatchDelayMs.toString());
  }

  if (batchSize !== undefined && batchSize > 0) {
    command.push('--batch-size', batchSize.toString());
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

