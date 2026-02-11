// Node definitions, derived from ansible/inventory.yml.
// Each node maps to a bare-metal host running Vector, which ships logs and metrics
// to CloudWatch (log group: /boundless/bento/<hostname>, namespace: Boundless/SystemMetrics).

export interface MonitoredNode {
    /** Short human-readable name used in resource IDs and alarm names. */
    name: string;
    /** Machine hostname as reported by Vector (ansible_hostname). */
    hostname: string;
    /** IP address of the node. */
    ip: string;
    /** Role: prover | explorer */
    role: "prover" | "explorer";
    /** Chain ID the node operates on. */
    chainId: string;
}

export interface EnvironmentConfig {
    nodes: MonitoredNode[];
}

// ---------------------------------------------------------------------------
// Staging
// ---------------------------------------------------------------------------
export const staging: EnvironmentConfig = {
    nodes: [
        {
            name: "explorer-84532-staging-release",
            hostname: "explorer-84532-staging-release",
            ip: "64.34.93.37",
            role: "explorer",
            chainId: "84532",
        },
        {
            name: "prover-84532-staging-nightly",
            hostname: "prover-84532-staging-nightly",
            ip: "64.34.84.17",
            role: "prover",
            chainId: "84532",
        },
    ],
};

// ---------------------------------------------------------------------------
// Production
// ---------------------------------------------------------------------------
export const production: EnvironmentConfig = {
    nodes: [
        {
            name: "explorer-8453-prod-release",
            hostname: "explorer-8453-production-release",
            ip: "186.233.186.43",
            role: "explorer",
            chainId: "8453",
        },
        {
            name: "prover-8453-prod-release",
            hostname: "prover-8453-production-release",
            ip: "64.34.93.87",
            role: "prover",
            chainId: "8453",
        },
        {
            name: "prover-8453-prod-nightly",
            hostname: "prover-8453-production-nightly",
            ip: "64.34.93.83",
            role: "prover",
            chainId: "8453",
        },
        {
            name: "prover-84532-prod-nightly",
            hostname: "prover-84532-production-nightly",
            ip: "160.202.129.195",
            role: "prover",
            chainId: "84532",
        },
        {
            name: "prover-11155111-prod-nightly",
            hostname: "prover-11155111-production-nightly",
            ip: "189.1.169.53",
            role: "prover",
            chainId: "11155111",
        },
    ],
};
