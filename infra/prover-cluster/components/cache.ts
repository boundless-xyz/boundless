import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export async function setupCache(
    name: string,
    network: any,
    tags: Record<string, string>,
    subnetIds?: pulumi.Input<pulumi.Input<string>[]>
) {
    const config = new pulumi.Config();
    const maxRedisSize = config.getNumber('rediSizeMax') ?? 100;
    const minRedisEcpus = config.getNumber('redisEcpuMin') ?? 1000;

    // Use provided subnets when co-locating with GPU instances; fallback to all private subnets
    const chosenSubnetIds = subnetIds ?? network.privateSubnetIds;

    // Create subnet group for ElastiCache
    const cacheSubnetGroup = new aws.elasticache.SubnetGroup(`${name}-cache-subnet-group`, {
        subnetIds: chosenSubnetIds,
        description: "Subnet group for Bento Redis",
        tags: {
            ...tags,
            Name: `${name}-cache-subnet-group`,
        },
    });

    const redis = new aws.elasticache.ServerlessCache(
        `${name}-redis`,
        {
            engine: 'valkey',
            name: `${name}-redis`,
            cacheUsageLimits: {
                dataStorage: {
                    maximum: maxRedisSize,
                    unit: 'GB',
                },
                ecpuPerSeconds: [
                    {
                        maximum: 15000000,
                        minimum: minRedisEcpus,
                    },
                ],
            },
            dailySnapshotTime: '09:00',
            description: 'workflow cache managed in redis',
            majorEngineVersion: '8',
            snapshotRetentionLimit: 1,
            securityGroupIds: [network.cacheSecurityGroup.id],
            // you are only allowed a max 3 subnets
            subnetIds: cacheSubnetGroup.subnetIds,
            tags: {
                ...tags,
                Name: `${name}-redis`,
            },
        },
        { ignoreChanges: ['subnetIds'], deleteBeforeReplace: true }
    );

    const redisEndpointHost = redis.endpoints.apply((endpoints) => endpoints[0].address);
    const redisEndpointPort = redis.endpoints.apply((endpoints) => endpoints[0].port);
    const redisEndpointUrl = pulumi.interpolate`rediss://${redisEndpointHost}:${redisEndpointPort}`;

    return {
        cluster: redis,
        connectionUrl: redisEndpointUrl,
    };
}