export enum ChainId {
  ETH_SEPOLIA = "11155111",
  BASE = "8453",
  BASE_SEPOLIA = "84532",
}

export const getChainName = (chainId: string | ChainId): string => {
  if (chainId === ChainId.ETH_SEPOLIA) {
    return "Ethereum Sepolia";
  }
  if (chainId === ChainId.BASE) {
    return "Base Mainnet";
  }
  if (chainId === "84532") {
    return "Base Sepolia";
  }
  throw new Error(`Invalid chain ID: ${chainId}`);
};

export const getChainId = (chainId: string): ChainId => {
  if (chainId === "11155111") {
    return ChainId.ETH_SEPOLIA;
  }
  if (chainId === "8453") {
    return ChainId.BASE;
  }
  if (chainId === "84532") {
    return ChainId.BASE_SEPOLIA;
  }
  throw new Error(`Invalid chain ID: ${chainId}`);
};

export enum Stage {
  STAGING = "staging",
  PROD = "prod",
}

export const getEnvVar = (name: string) => {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Environment variable ${name} is not set`);
  }
  return value;
};

// Returns a service name for naming resources.
// NOTE: Do not modify this function as it will affect existing resources, causing them to be renamed
//       and recreated. This is because the service name is used as part of each resource name.
//       
//       To use a new naming scheme for new services, we should create a new "V2" function.
export const getServiceNameV1 = (stackName: string, name: string, chainId?: ChainId | string) => {
  const isDev = stackName === "dev";
  const prefix = isDev ? `${getEnvVar("DEV_NAME")}` : `${stackName}`;
  const suffix = chainId ? `-${chainId}` : "";
  const serviceName = `${prefix}-${name}${suffix}`;
  return serviceName;
};

// Severity levels for alarms. The strings here are detected in PageDuty and used to
// create the severity of the PagerDuty incident.
export enum Severity {
  SEV1 = 'SEV1',
  SEV2 = 'SEV2',
}
