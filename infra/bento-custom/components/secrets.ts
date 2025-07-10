import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export async function setupSecrets(
    name: string,
    tags: Record<string, string>
) {
    const config = new pulumi.Config();

    // Create secret for broker private key
    const brokerPrivateKey = new aws.secretsmanager.Secret(`${name}-broker-private-key`, {
        name: `${name}/broker/private-key`,
        description: "Private key for broker wallet",
        tags: {
            ...tags,
            Name: `${name}-broker-private-key`,
        },
    });

    // Set the actual secret value (must be provided via Pulumi config)
    const brokerPrivateKeyVersion = new aws.secretsmanager.SecretVersion(`${name}-broker-private-key-version`, {
        secretId: brokerPrivateKey.id,
        secretString: config.requireSecret("brokerPrivateKey"),
    });

    // Create secret for RPC URL
    const rpcUrl = new aws.secretsmanager.Secret(`${name}-rpc-url`, {
        name: `${name}/broker/rpc-url`,
        description: "RPC URL for blockchain access",
        tags: {
            ...tags,
            Name: `${name}-rpc-url`,
        },
    });

    const rpcUrlVersion = new aws.secretsmanager.SecretVersion(`${name}-rpc-url-version`, {
        secretId: rpcUrl.id,
        secretString: config.requireSecret("rpcUrl"),
    });

    // S3 credentials for Bento (using IAM roles instead)
    // We'll use IAM instance profiles for S3 access

    return {
        brokerPrivateKey: brokerPrivateKey.arn,
        rpcUrl: rpcUrl.arn,
    };
}