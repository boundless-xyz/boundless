const logGroupMapping = (service: string): string => {
  switch (service) {
    case 'bento-prover':
      return 'prod-11155111-bento-prover-11155111';
    case 'bonsai-prover':
      return 'prod-11155111-bonsai-prover-11155111';
    case 'monitor':
      return 'prod-11155111-monitor-11155111-monitor-lambda';
    case 'indexer':
      return 'prod-11155111-indexer-11155111-service';
    case 'og-offchain':
      return 'prod-11155111-og-offchain-11155111';
    case 'og-onchain':
      return 'prod-11155111-og-onchain-11155111';
    case 'order-stream':
      return 'prod-11155111-order-stream';
    case 'order-slasher':
      return 'prod-11155111-order-slasher-11155111';
    default:
      console.log(`No log group name found for service: ${service}. Attempting to infer from service name.`);
      return `prod-11155111-${service}-11155111`;
  }
}

export const getLogGroupName = (stage: string, chainId: string, service: string) => {
  let logGroupName = logGroupMapping(service);
  if (!logGroupName) {
    throw new Error(`No log group name found for service: ${service}`);
  }
  logGroupName = logGroupName.replace(/11155111/g, chainId);
  logGroupName = logGroupName.replace(/prod/g, stage);
  console.log(`Log group name for ${stage} and chainId ${chainId} and service ${service}: ${logGroupName}`);
  return logGroupName;
} 