export enum ChainId {
  ETH_MAINNET = "1",
  ETH_SEPOLIA = "11155111",
  BASE = "8453",
  BASE_SEPOLIA = "84532",
  TAIKO = "167000",
}

export const getChainName = (chainId: string | ChainId): string => {
  if (chainId === ChainId.ETH_MAINNET) {
    return "Ethereum Mainnet";
  }
  if (chainId === ChainId.ETH_SEPOLIA) {
    return "Ethereum Sepolia";
  }
  if (chainId === ChainId.BASE) {
    return "Base Mainnet";
  }
  if (chainId === "84532") {
    return "Base Sepolia";
  }
  if (chainId === ChainId.TAIKO) {
    return "Taiko";
  }
  throw new Error(`Invalid chain ID: ${chainId}`);
};

export const getChainId = (chainId: string): ChainId => {
  if (chainId === "1") {
    return ChainId.ETH_MAINNET;
  }
  if (chainId === "11155111") {
    return ChainId.ETH_SEPOLIA;
  }
  if (chainId === "8453") {
    return ChainId.BASE;
  }
  if (chainId === "84532") {
    return ChainId.BASE_SEPOLIA;
  }
  if (chainId === "167000") {
    return ChainId.TAIKO;
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
  const prefix = process.env.DEV_NAME || isDev ? `${getEnvVar("DEV_NAME")}` : `${stackName}`;
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

export const GHCR_IMAGE_PREFIX = 'ghcr.io/boundless-xyz/boundless';

/**
 * Constructs a GHCR image URI from the current git SHA and waits
 * for it to exist (CI may still be building). Retries with backoff
 * for up to ~15 minutes before failing.
 */
export async function getGhcrImageUri(serviceName: string): Promise<string> {
  const sha = require('child_process')
    .execSync('git rev-parse --short HEAD')
    .toString().trim();
  const tag = `nightly-${sha}`;
  const uri = `${GHCR_IMAGE_PREFIX}/${serviceName}:${tag}`;

  // Check GHCR manifest via registry API (public, no auth needed)
  const url = `https://ghcr.io/v2/boundless-xyz/boundless/${serviceName}/manifests/${tag}`;
  const maxAttempts = 30;      // 30 attempts
  const intervalMs = 30_000;   // 30s between attempts → ~15 min total

  for (let i = 1; i <= maxAttempts; i++) {
    try {
      const res = require('child_process').execSync(
        `curl -sf -o /dev/null -w "%{http_code}" -H "Accept: application/vnd.oci.image.index.v1+json" "${url}"`,
        { encoding: 'utf-8' }
      ).trim();
      if (res === '200') {
        console.log(`GHCR image found: ${uri}`);
        return uri;
      }
    } catch { /* curl returns non-zero on 404 */ }

    if (i < maxAttempts) {
      console.log(`Waiting for GHCR image ${uri} (attempt ${i}/${maxAttempts}, retrying in 30s)...`);
      require('child_process').execSync(`sleep 30`);
    }
  }
  throw new Error(`GHCR image not found after ${maxAttempts} attempts: ${uri}`);
}

export const DEPLOYMENT_ROLE_MAX_SESSION_DURATION_SECONDS = 7200;

/** Max session duration when assuming a role from another role (role chaining). AWS limit is 1 hour. */
export const ASSUME_ROLE_CHAINED_MAX_SESSION_SECONDS = 3600;
