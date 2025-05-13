import * as pulumi from "@pulumi/pulumi";
import raw from "../targets.json";

export interface Target {
    name: string;
    address: string;
    // Optional: e.g., one submission every 5 minutes -> 300
    submissionRate?: number;
    // Optional: e.g., 90% success rate -> 0.9
    successRate?: number;
}

const stackName = pulumi.getStack();
const envKey = (stackName === "staging" || stackName === "dev") ? "staging" : "prod";

const envData = raw[envKey] as {
    clients: Target[];
    provers: Target[];
};

export const clients: Target[] = envData.clients;
export const provers: Target[] = envData.provers;

export const clientAddresses: string[] = clients.map(c => c.address);
export const proverAddresses: string[] = provers.map(p => p.address);
