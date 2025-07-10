import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export async function setupCache(
    name: string,
    network: any,
    tags: Record<string, string>,
    subnetIds?: pulumi.Input<pulumi.Input<string>[]>
) {
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

    // Create ElastiCache Redis cluster
    const redisCluster = new aws.elasticache.Cluster(`${name}-redis`, {
        engine: "redis",
        engineVersion: "7.0",
        nodeType: "cache.r7g.large",
        numCacheNodes: 1,

        subnetGroupName: cacheSubnetGroup.name,
        securityGroupIds: [network.cacheSecurityGroup.id],

        snapshotRetentionLimit: 5,
        snapshotWindow: "03:00-05:00",
        maintenanceWindow: "sun:05:00-sun:07:00",

        parameterGroupName: "default.redis7",

        // Enable automatic failover for production
        // automaticFailoverEnabled: true,

        tags: {
            ...tags,
            Name: `${name}-redis`,
        },
    });

    return {
        cluster: redisCluster,
        endpoint: redisCluster.cacheNodes[0].address,
        connectionUrl: pulumi.interpolate`redis://${redisCluster.cacheNodes[0].address}:6379`,
    };
}