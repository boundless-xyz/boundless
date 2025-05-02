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
  // When creating S3 buckets using prefixName the max length is 37 characters.
  if (serviceName.length > 37) {
    throw new Error(`Service name ${serviceName} is too long`);
  }
  return serviceName;
};

// Severity levels for alarms. The strings here are detected in PageDuty and used to
// create the severity of the PagerDuty incident.
export enum Severity {
  SEV1 = 'SEV1',
  SEV2 = 'SEV2',
}
