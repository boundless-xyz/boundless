import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

// Create a secrets manager secret for the database password
function createDatabaseSecret(
    name: string,
    password: pulumi.Output<string>,
    databaseEndpoint: pulumi.Output<string>,
    tags: Record<string, string>
) {
    const secret = new aws.secretsmanager.Secret(`${name}-db-secret`, {
        description: "Database credentials for Bento PostgreSQL",
        recoveryWindowInDays: 0, // TODO(ec2): fixme
        tags: {
            ...tags,
            Name: `${name}-db-secret`,
        },
    });

    // Create the secret version with the password
    return new aws.secretsmanager.SecretVersion(`${name}-db-secret-version`, {
        secretId: secret.id,
        secretString: pulumi.all([password, databaseEndpoint]).apply(([pwd, endpoint]) =>
            JSON.stringify({
                username: "worker",
                password: pwd,
            })
        ),
    });
}

export async function setupDatabase(
    name: string,
    network: any,
    tags: Record<string, string>,
    rdsPassword: pulumi.Output<string>
) {
    // Create subnet group for RDS
    const dbSubnetGroup = new aws.rds.SubnetGroup(`${name}-db-subnet-group`, {
        subnetIds: network.privateSubnetIds,
        description: "Subnet group for Bento PostgreSQL",
        tags: {
            ...tags,
            Name: `${name}-db-subnet-group`,
        },
    });

    const taskdbParameterGroup = new aws.rds.ParameterGroup(
        `${name}-db-params`,
        {
            family: 'postgres17',
            description: 'Parameter group for taskdb with logical replication enabled',
            parameters: [
                {
                    name: 'synchronous_commit',
                    value: 'off',
                    applyMethod: 'pending-reboot',
                },
            ],
        },
    );

    // Create PostgreSQL instance
    const database = new aws.rds.Instance(`${name}-postgres`, {
        identifier: `${name}-postgres`,
        engine: "postgres",
        engineVersion: "17.4",
        instanceClass: "db.m7g.large",
        allocatedStorage: 20,
        maxAllocatedStorage: 500,
        storageType: "gp3",
        storageEncrypted: true,
        publiclyAccessible: false,

        dbName: "taskdb",
        username: "worker",
        password: rdsPassword,

        vpcSecurityGroupIds: [network.databaseSecurityGroup.id],
        dbSubnetGroupName: dbSubnetGroup.name,

        skipFinalSnapshot: true, // Set to false in production
        deletionProtection: false, // Set to true in production

        backupRetentionPeriod: 7,
        backupWindow: "03:00-04:00",
        maintenanceWindow: "sun:04:00-sun:05:00",

        // Asynchronous WAL through parameter group
        parameterGroupName: taskdbParameterGroup.name,
        performanceInsightsEnabled: true,

        enabledCloudwatchLogsExports: ["postgresql"],

        tags: {
            ...tags,
            Name: `${name}-postgres`,
        },
    });

    // Create the database secret first
    const databaseSecret = createDatabaseSecret(name, rdsPassword, database.endpoint, tags);

    // Create RDS Proxy for better connection management
    const rdsProxy = new aws.rds.Proxy(`${name}-rds-proxy`, {
        name: `${name}-rds-proxy`,
        engineFamily: "POSTGRESQL",
        roleArn: network.rdsProxyRole.arn,
        vpcSubnetIds: network.privateSubnetIds,
        vpcSecurityGroupIds: [network.rdsProxySecurityGroup.id],
        idleClientTimeout: 1800, // 30 minutes
        debugLogging: false,

        // Authentication will be set up via target group
        auths: [{
            authScheme: "SECRETS",
            // Use the configured RDS password
            iamAuth: "DISABLED",
            secretArn: databaseSecret.arn,
        }],

        // Require TLS for secure connections (matching order stream)
        requireTls: true,

        tags: {
            ...tags,
            Name: `${name}-rds-proxy`,
        },
    });

    // Create proxy target group
    const proxyTargetGroup = new aws.rds.ProxyDefaultTargetGroup(`${name}-proxy-target-group`, {
        dbProxyName: rdsProxy.name,
        connectionPoolConfig: {
            maxConnectionsPercent: 100,
            maxIdleConnectionsPercent: 10,
            connectionBorrowTimeout: 120,
        },
    });

    // Add the database instance as a target
    const proxyTarget = new aws.rds.ProxyTarget(`${name}-proxy-target`, {
        dbProxyName: rdsProxy.name,
        targetGroupName: proxyTargetGroup.name,
        dbInstanceIdentifier: database.identifier,
    });

    return {
        instance: database,
        proxy: rdsProxy,
        endpoint: database.endpoint,
        proxyEndpoint: rdsProxy.endpoint,
        // Use RDS proxy connection with configured password and SSL mode
        connectionUrl: pulumi.all([rdsPassword, rdsProxy.endpoint]).apply(([password, endpoint]) => {
            return pulumi.interpolate`postgresql://worker:${password}@${rdsProxy.endpoint}:5432/taskdb?sslmode=require`;
        }),
        secret: databaseSecret,
    };
}