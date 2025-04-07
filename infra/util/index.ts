export enum ChainId {
  SEPOLIA = "11155111",
}

export const getEnvVar = (name: string) => {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Environment variable ${name} is not set`);
  }
  return value;
};

export const getServiceName = (stackName: string, name: string, chainId: ChainId) => {
  const isDev = stackName === "dev";
  const prefix = isDev ? `${getEnvVar("DEV_NAME")}` : `${stackName}`;
  return `${prefix}-${name}-${chainId}`;
};

