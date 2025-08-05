import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
export type Secrets = {
    brokerPrivateKey: pulumi.Output<string>;
    rpcUrl: pulumi.Output<string>;
    dockerToken: pulumi.Output<string>;
    orderStreamUrl: pulumi.Output<string>;
};

export async function setupSecrets(
    name: string,
    tags: Record<string, string>
): Promise<Secrets> {
    const config = new pulumi.Config();

    // Create secret for broker private key
    const brokerPrivateKey = new aws.secretsmanager.Secret(`${name}-broker-private-key-1`, {
        name: `${name}/broker/private-key-1`,
        description: "Private key for broker wallet",
        recoveryWindowInDays: 0, // TODO(ec2): fixme
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
    const rpcUrl = new aws.secretsmanager.Secret(`${name}-rpc-url-1`, {
        name: `${name}/broker/rpc-url-1`,
        description: "RPC URL for blockchain access",
        recoveryWindowInDays: 0, // TODO(ec2): fixme
        tags: {
            ...tags,
            Name: `${name}-rpc-url`,
        },
    });

    const rpcUrlVersion = new aws.secretsmanager.SecretVersion(`${name}-rpc-url-version`, {
        secretId: rpcUrl.id,
        secretString: config.requireSecret("rpcUrl"),
    });


    // Create secret for docker token
    const dockerToken = new aws.secretsmanager.Secret(`${name}-docker-token-1`, {
        name: `${name}/broker/docker-token-1`,
        description: "Docker token for image access",
        recoveryWindowInDays: 0, // TODO(ec2): fixme
        tags: {
            ...tags,
            Name: `${name}-docker-token`,
        },
    });

    const dockerTokenVersion = new aws.secretsmanager.SecretVersion(`${name}-docker-token-version`, {
        secretId: dockerToken.id,
        secretString: config.requireSecret("dockerToken"),
    });

    // Create secret for order stream URL
    const orderStreamUrl = new aws.secretsmanager.Secret(`${name}-order-stream-url-1`, {
        name: `${name}/broker/order-stream-url-1`,
        description: "Order stream URL for broker",
        recoveryWindowInDays: 0, // TODO(ec2): fixme
        tags: {
            ...tags,
            Name: `${name}-order-stream-url`,
        },
    });

    const orderStreamUrlVersion = new aws.secretsmanager.SecretVersion(`${name}-order-stream-url-version`, {
        secretId: orderStreamUrl.id,
        secretString: config.requireSecret("orderStreamUrl"),
    });

    // S3 credentials for Bento (using IAM roles instead)
    // We'll use IAM instance profiles for S3 access

    return {
        brokerPrivateKey: brokerPrivateKey.arn,
        rpcUrl: rpcUrl.arn,
        dockerToken: dockerToken.arn,
        orderStreamUrl: orderStreamUrl.arn,
    };
}
