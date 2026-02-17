import { ECSClient, RunTaskCommand, StopTaskCommand } from '@aws-sdk/client-ecs';
import { Handler, Context } from 'aws-lambda';

interface LoadTestEvent {
  action: 'start' | 'cancel';
  // Start params (sent by the trigger script with defaults applied)
  interval?: number;
  count?: number;
  submitOffchain?: boolean;
  inputMaxMCycles?: number;
  min?: string;
  max?: string;
  rampUp?: number;
  lockTimeout?: number;
  timeout?: number;
  secondsPerMCycle?: number;
  rampUpSecondsPerMCycle?: number;
  execRateKhz?: number;
  lockCollateralRaw?: string;
  txTimeout?: number;
  // Cancel params
  taskArn?: string;
}

interface LoadTestResponse {
  taskArn?: string;
  message: string;
  error?: string;
}

export const handler: Handler<LoadTestEvent, LoadTestResponse> = async (
  event: LoadTestEvent,
  context: Context
): Promise<LoadTestResponse> => {
  console.log('Received load test request', { event, context });

  const requiredEnvVars = [
    'CLUSTER_ARN',
    'TASK_DEFINITION_ARN',
    'SUBNET_IDS',
    'SECURITY_GROUP_ID',
    'CONTAINER_NAME',
    'BOUNDLESS_ADDRESS',
    'SET_VERIFIER_ADDRESS',
  ];

  for (const envVar of requiredEnvVars) {
    if (!process.env[envVar]) {
      throw new Error(`Missing required environment variable: ${envVar}`);
    }
  }

  const ecsClient = new ECSClient({ region: process.env.AWS_REGION || 'us-west-2' });

  if (event.action === 'cancel') {
    return handleCancel(ecsClient, event);
  }

  if (event.action === 'start') {
    return handleStart(ecsClient, event);
  }

  return {
    message: 'Invalid action',
    error: 'action must be "start" or "cancel"',
  };
};

async function handleCancel(
  ecsClient: ECSClient,
  event: LoadTestEvent
): Promise<LoadTestResponse> {
  if (!event.taskArn) {
    return {
      message: 'Missing required parameter',
      error: 'taskArn is required for cancel action',
    };
  }

  try {
    await ecsClient.send(
      new StopTaskCommand({
        cluster: process.env.CLUSTER_ARN,
        task: event.taskArn,
        reason: 'Load test cancelled via Lambda',
      })
    );

    console.log('Task stopped successfully:', event.taskArn);
    return {
      taskArn: event.taskArn,
      message: 'Load test task cancelled successfully',
    };
  } catch (error) {
    console.error('Error stopping ECS task:', error);
    return {
      message: 'Error cancelling task',
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

async function handleStart(
  ecsClient: ECSClient,
  event: LoadTestEvent
): Promise<LoadTestResponse> {
  // Build command from event payload. The trigger script is responsible for
  // applying defaults, so the Lambda just passes through what it receives.
  const command: string[] = [
    '--boundless-market-address', process.env.BOUNDLESS_ADDRESS!,
    '--set-verifier-address', process.env.SET_VERIFIER_ADDRESS!,
  ];

  if (event.interval !== undefined) {
    command.push('--interval', event.interval.toString());
  }
  if (event.count !== undefined) {
    command.push('--count', event.count.toString());
  }
  if (event.submitOffchain) {
    command.push('--submit-offchain');
  }
  if (event.inputMaxMCycles !== undefined) {
    command.push('--input-max-mcycles', event.inputMaxMCycles.toString());
  }
  if (event.min !== undefined) {
    command.push('--min', event.min);
  }
  if (event.max !== undefined) {
    command.push('--max', event.max);
  }
  if (event.rampUp !== undefined) {
    command.push('--ramp-up', event.rampUp.toString());
  }
  if (event.lockTimeout !== undefined) {
    command.push('--lock-timeout', event.lockTimeout.toString());
  }
  if (event.timeout !== undefined) {
    command.push('--timeout', event.timeout.toString());
  }
  if (event.secondsPerMCycle !== undefined) {
    command.push('--seconds-per-mcycle', event.secondsPerMCycle.toString());
  }
  if (event.rampUpSecondsPerMCycle !== undefined) {
    command.push('--ramp-up-seconds-per-mcycle', event.rampUpSecondsPerMCycle.toString());
  }
  if (event.execRateKhz !== undefined) {
    command.push('--exec-rate-khz', event.execRateKhz.toString());
  }
  if (event.lockCollateralRaw !== undefined) {
    command.push('--lock-collateral-raw', event.lockCollateralRaw);
  }
  if (event.txTimeout !== undefined) {
    command.push('--tx-timeout', event.txTimeout.toString());
  }

  console.log('Running task with command:', command);

  const subnets = process.env.SUBNET_IDS!.split(',');

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
      message: 'Load test task started successfully',
    };
  } catch (error) {
    console.error('Error starting ECS task:', error);
    return {
      message: 'Error starting task',
      error: error instanceof Error ? error.message : String(error),
    };
  }
}
