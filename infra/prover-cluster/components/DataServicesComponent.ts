import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface DataServicesComponentConfig extends BaseComponentConfig {
    taskDBName: string;
    taskDBUsername: string;
    taskDBPassword: string;
    securityGroupId: pulumi.Output<string>;
    rdsInstanceClass?: string;
}

export class DataServicesComponent extends BaseComponent {
    public readonly rdsInstance: aws.rds.Instance;
    public readonly rdsEndpoint: pulumi.Output<string>;
    public readonly s3Bucket: aws.s3.Bucket;
    public readonly s3BucketName: pulumi.Output<string>;
    public readonly dbSubnetGroup: aws.rds.SubnetGroup;
    public readonly rdsSecurityGroup: aws.ec2.SecurityGroup;

    constructor(config: DataServicesComponentConfig) {
        super(config, "boundless-bento");

        // Create DB subnet group
        this.dbSubnetGroup = new aws.rds.SubnetGroup(`${config.stackName}-db-subnet-group`, {
            name: this.generateName("db-subnet-group"),
            subnetIds: config.privateSubnetIds,
            tags: {
                Environment: config.environment,
                Stack: config.stackName,
                Component: "data-services",
            },
        });

        // Create RDS security group
        this.rdsSecurityGroup = new aws.ec2.SecurityGroup(`${config.stackName}-rds-sg`, {
            name: this.generateName("rds-sg"),
            vpcId: config.vpcId,
            description: "Security group for RDS PostgreSQL",
            ingress: [{
                protocol: "tcp",
                fromPort: 5432,
                toPort: 5432,
                securityGroups: [config.securityGroupId],
                description: "PostgreSQL access from cluster instances",
            }],
            egress: [{
                protocol: "-1",
                fromPort: 0,
                toPort: 0,
                cidrBlocks: ["0.0.0.0/0"],
                description: "All outbound traffic",
            }],
            tags: {
                Environment: config.environment,
                Stack: config.stackName,
                Component: "rds",
            },
        });

        // Create RDS PostgreSQL instance
        this.rdsInstance = new aws.rds.Instance(`${config.stackName}`, {
            identifier: this.generateName("postgres"),
            engine: "postgres",
            engineVersion: "17.4",
            instanceClass: config.rdsInstanceClass || "db.t4g.micro",
            allocatedStorage: 20,
            maxAllocatedStorage: 100,
            storageType: "gp3",
            storageEncrypted: true,
            dbName: config.taskDBName,
            username: config.taskDBUsername,
            password: config.taskDBPassword,
            port: 5432,
            publiclyAccessible: false,
            dbSubnetGroupName: this.dbSubnetGroup.name,
            vpcSecurityGroupIds: [this.rdsSecurityGroup.id],
            skipFinalSnapshot: true,
            backupRetentionPeriod: 7,
            tags: {
                Environment: config.environment,
                Stack: config.stackName,
                Component: "postgres",
            },
        });

        // RDS endpoint is just the hostname, need to append port
        this.rdsEndpoint = pulumi.interpolate`${this.rdsInstance.endpoint}:${this.rdsInstance.port}`;

        // Create S3 bucket for workflow storage
        const bucketName = this.generateName("bento-storage");
        this.s3Bucket = new aws.s3.Bucket(`${config.stackName}-storage`, {
            bucket: bucketName,
            forceDestroy: false, // Prevent accidental deletion
            tags: {
                Environment: config.environment,
                Stack: config.stackName,
                Component: "s3-storage",
            },
        });

        // Block public access
        new aws.s3.BucketPublicAccessBlock(`${config.stackName}-storage-pab`, {
            bucket: this.s3Bucket.id,
            blockPublicAcls: true,
            blockPublicPolicy: true,
            ignorePublicAcls: true,
            restrictPublicBuckets: true,
        });

        this.s3BucketName = this.s3Bucket.id;
    }
}

