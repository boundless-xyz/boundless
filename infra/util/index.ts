import * as pulumi from "@pulumi/pulumi";

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

export const getServiceName = (name: string, chainId: ChainId) => {
  const stackName = pulumi.getStack();
  const isDev = stackName === "dev";
  const prefix = isDev ? `${getEnvVar("DEV_NAME")}` : `${stackName}`;
  return `${prefix}-${name}-${chainId}`;
};

