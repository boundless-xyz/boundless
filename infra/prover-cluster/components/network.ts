import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as awsx from "@pulumi/awsx";

export type Network = {
    vpc: awsx.ec2.Vpc;
    publicSubnetIds: pulumi.Output<string[]>;
    privateSubnetIds: pulumi.Output<string[]>;
    instanceSecurityGroup: aws.ec2.SecurityGroup;
    brokerSecurityGroup: aws.ec2.SecurityGroup;
    databaseSecurityGroup: aws.ec2.SecurityGroup;
    cacheSecurityGroup: aws.ec2.SecurityGroup;
    rdsProxySecurityGroup: aws.ec2.SecurityGroup;
    rdsProxyRole: aws.iam.Role;
    s3VpcEndpoint: aws.ec2.VpcEndpoint;
    getPrimaryPublicSubnetId: () => pulumi.Output<string>;
    getPrimaryPrivateSubnetId: () => pulumi.Output<string>;
};

export async function setupNetwork(name: string, tags: Record<string, string>): Promise<Network> {
    // Create VPC with public and private subnets
    const vpc = new awsx.ec2.Vpc(`${name}-vpc`, {
        cidrBlock: "10.0.0.0/16",
        numberOfAvailabilityZones: 2,
        enableDnsHostnames: true,
        enableDnsSupport: true,
        natGateways: {
            strategy: "Single", // Use single NAT gateway to save costs
        },
        tags: {
            ...tags,
            Name: `${name}-vpc`,
        },
    });

    // Security group for EC2 instances
    const instanceSecurityGroup = new aws.ec2.SecurityGroup(`${name}-instance-sg`, {
        vpcId: vpc.vpcId,
        description: "Security group for Bento instances",
        ingress: [
            // SSH access
            {
                protocol: "tcp",
                fromPort: 22,
                toPort: 22,
                cidrBlocks: ["0.0.0.0/0"],
            },
            // Internal communication
            {
                protocol: "-1",
                fromPort: 0,
                toPort: 0,
                self: true,
            },
        ],
        egress: [
            {
                protocol: "-1",
                fromPort: 0,
                toPort: 0,
                cidrBlocks: ["0.0.0.0/0"],
            },
        ],
        tags: {
            ...tags,
            Name: `${name}-instance-sg`,
        },
    });



    // Security group for broker EC2 instance (doesn't need database access)
    const brokerSecurityGroup = new aws.ec2.SecurityGroup(`${name}-broker-sg`, {
        vpcId: vpc.vpcId,
        description: "Security group for broker EC2 instance",
        ingress: [
            {
                protocol: "tcp",
                fromPort: 22,
                toPort: 22,
                cidrBlocks: ["0.0.0.0/0"],
                description: "SSH access",
            },
            {
                protocol: "-1",
                fromPort: 0,
                toPort: 0,
                self: true,
                description: "Allow internal communication",
            },
        ],
        egress: [
            {
                protocol: "-1",
                fromPort: 0,
                toPort: 0,
                cidrBlocks: ["0.0.0.0/0"],
            },
        ],
        tags: {
            ...tags,
            Name: `${name}-broker-sg`,
        },
    });

    // Security group for ElastiCache
    const cacheSecurityGroup = new aws.ec2.SecurityGroup(`${name}-cache-sg`, {
        vpcId: vpc.vpcId,
        description: "Security group for ElastiCache Redis",
        ingress: [
            {
                protocol: "tcp",
                fromPort: 6379,
                toPort: 6379,
                securityGroups: [instanceSecurityGroup.id],
                description: "Allow Redis from EC2 instances",
            },
        ],
        egress: [
            {
                protocol: "-1",
                fromPort: 0,
                toPort: 0,
                cidrBlocks: ["0.0.0.0/0"],
            },
        ],
        tags: {
            ...tags,
            Name: `${name}-cache-sg`,
        },
    });

    // Allow broker to access Bento API (port 8081)
    new aws.ec2.SecurityGroupRule(`${name}-bento-api-from-broker`, {
        type: "ingress",
        fromPort: 8081,
        toPort: 8081,
        protocol: "tcp",
        securityGroupId: instanceSecurityGroup.id,
        sourceSecurityGroupId: brokerSecurityGroup.id,
        description: "Allow Bento API access from broker",
    });

    // Security group for RDS Proxy
    const rdsProxySecurityGroup = new aws.ec2.SecurityGroup(`${name}-rds-proxy-sg`, {
        vpcId: vpc.vpcId,
        description: "Security group for RDS Proxy",
        ingress: [
            {
                protocol: "tcp",
                fromPort: 5432,
                toPort: 5432,
                // securityGroups: [instanceSecurityGroup.id],
                cidrBlocks: ['0.0.0.0/0'],
                description: "Allow PostgreSQL from EC2 instances",
            },
        ],
        egress: [
            {
                protocol: '-1',
                fromPort: 0,
                toPort: 0,
                cidrBlocks: ['0.0.0.0/0'],
            },
        ],
        tags: {
            ...tags,
            Name: `${name}-rds-proxy-sg`,
        },
    });

    // Security group for RDS
    const databaseSecurityGroup = new aws.ec2.SecurityGroup(`${name}-db-sg`, {
        vpcId: vpc.vpcId,
        description: "Security group for RDS PostgreSQL",
        ingress: [
            {
                protocol: "tcp",
                fromPort: 5432,
                toPort: 5432,
                securityGroups: [rdsProxySecurityGroup.id],
                description: "Allow connections from proxy ",
            },
            {
                protocol: "tcp",
                fromPort: 5432,
                toPort: 5432,
                securityGroups: [instanceSecurityGroup.id],
                description: "Allow connections from EC2 instances",
            },
        ],
        egress: [
            {
                protocol: "-1",
                fromPort: 0,
                toPort: 0,
                cidrBlocks: ["0.0.0.0/0"],
            },
        ],
        tags: {
            ...tags,
            Name: `${name}-db-sg`,
        },
    });

    // IAM role for RDS Proxy
    const rdsProxyRole = new aws.iam.Role(`${name}-rds-proxy-role`, {
        assumeRolePolicy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [
                {
                    Action: "sts:AssumeRole",
                    Effect: "Allow",
                    Principal: {
                        Service: "rds.amazonaws.com",
                    },
                },
            ],
        }),
        tags: {
            ...tags,
            Name: `${name}-rds-proxy-role`,
        },
    });

    // Policy for RDS Proxy to access Secrets Manager
    const rdsProxyPolicy = new aws.iam.RolePolicy(`${name}-rds-proxy-policy`, {
        role: rdsProxyRole.id,
        policy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [
                {
                    Effect: "Allow",
                    Action: [
                        "secretsmanager:GetSecretValue",
                        "secretsmanager:DescribeSecret",
                    ],
                    Resource: "*",
                },
            ],
        }),
    });


    // Create VPC endpoints for private subnet access to AWS services
    // Derive the route-table IDs associated with each private subnet. This avoids the
    // heavier getRouteTables call that occasionally times-out during previews.
    const privateRouteTables = vpc.privateSubnetIds.apply(async (ids) => {
        const tableLookups = await Promise.all(ids.map((id) =>
            aws.ec2.getRouteTable({
                filters: [{ name: "association.subnet-id", values: [id] }],
            })
        ));
        return tableLookups.map(t => t.id);
    });

    // Get current region
    const currentRegion = aws.getRegion();

    // S3 VPC Endpoint (Gateway endpoint)
    const s3VpcEndpoint = new aws.ec2.VpcEndpoint(`${name}-s3-vpc-endpoint`, {
        vpcId: vpc.vpcId,
        serviceName: pulumi.output(currentRegion).apply(r => `com.amazonaws.${r.name}.s3`),
        routeTableIds: privateRouteTables,
        tags: {
            ...tags,
            Name: `${name}-s3-endpoint`,
        },
    });


    return {
        vpc,
        publicSubnetIds: vpc.publicSubnetIds,
        privateSubnetIds: vpc.privateSubnetIds,
        instanceSecurityGroup,
        brokerSecurityGroup,
        databaseSecurityGroup,
        cacheSecurityGroup,
        rdsProxySecurityGroup,
        rdsProxyRole,
        s3VpcEndpoint,
        // Helper function to get the first public subnet for co-location
        getPrimaryPublicSubnetId: () => vpc.publicSubnetIds[0],
        // Helper function to get the first private subnet for co-location
        getPrimaryPrivateSubnetId: () => vpc.privateSubnetIds[0],
    };
}