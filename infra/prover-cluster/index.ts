import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import {
    SecurityComponent,
    ManagerComponent,
    WorkerClusterComponent,
    ApiGatewayComponent,
    DataServicesComponent,
    BaseComponentConfig
} from "./components";

const stackName = pulumi.getStack();
const baseConfig = new pulumi.Config("base");
const baseStackName = baseConfig.require('BASE_STACK');
const baseStack = new pulumi.StackReference(baseStackName);
const vpcId = baseStack.getOutput('VPC_ID') as pulumi.Output<string>;
const privSubNetIds = baseStack.getOutput('PRIVATE_SUBNET_IDS') as pulumi.Output<string[]>;
const pubSubNetIds = baseStack.getOutput('PUBLIC_SUBNET_IDS') as pulumi.Output<string[]>;

const config = new pulumi.Config();

// Configuration with proper types
const environment: string = config.get("environment") || "custom";

// Required configuration
const privateKey: pulumi.Output<string> = config.requireSecret("privateKey");
const ethRpcUrl: pulumi.Output<string> = config.requireSecret("ethRpcUrl");
const managerInstanceType: string = config.require("managerInstanceType");
const orderStreamUrl: string = config.require("orderStreamUrl");
const verifierAddress: string = config.require("verifierAddress");
const boundlessMarketAddress: string = config.require("boundlessMarketAddress");
const setVerifierAddress: string = config.require("setVerifierAddress");
const collateralTokenAddress: string = config.require("collateralTokenAddress");
const chainId: string = config.require("chainId");
const apiKey: pulumi.Output<string> = config.requireSecret("apiKey");

// DB vars
const taskDBUsername: string = config.require("taskDBUsername");
const taskDBPassword: string = config.require("taskDBPassword");
const taskDBName: string = config.require("taskDBName");

// Worker counts
const executionCount: number = config.getNumber("executionCount") || 1;
const proverCount: number = config.getNumber("proverWorkerCount") || 1;
const auxCount: number = config.getNumber("auxWorkerCount") || 1;

// Broker configuration
const mcyclePrice: string = config.get("mcyclePrice") || "0.00000001";
const peakProveKhz: number = config.getNumber("peakProveKhz") || 100;
const minDeadline: number = config.getNumber("minDeadline") || 0;
const lookbackBlocks: number = config.getNumber("lookbackBlocks") || 0;
const maxCollateral: string = config.get("maxCollateral") || "200";
const maxFileSize: string = config.get("maxFileSize") || "0";
const maxMcycleLimit: string = config.get("maxMcycleLimit") || "0";
const maxConcurrentProofs: number = config.getNumber("maxConcurrentProofs") || 1;
const balanceWarnThreshold: string = config.get("balanceWarnThreshold") || "0";
const balanceErrorThreshold: string = config.get("balanceErrorThreshold") || "0";
const collateralBalanceWarnThreshold: string = config.get("collateralBalanceWarnThreshold") || "0";
const collateralBalanceErrorThreshold: string = config.get("collateralBalanceErrorThreshold") || "0";
const priorityRequestorAddresses: string = config.get("priorityRequestorAddresses") || "";
const denyRequestorAddresses: string = config.get("denyRequestorAddresses") || "";
const maxFetchRetries: number = config.getNumber("maxFetchRetries") || 3;
const allowClientAddresses: string = config.get("allowClientAddresses") || "";
const lockinPriorityGas: string = config.get("lockinPriorityGas") || "0";

// Look up the latest packer-built AMI
const boundlessBentoVersion: string = config.get("boundlessBentoVersion") || "nightly";
const boundlessAmiName: string = config.get("boundlessAmiName") || `boundless-${boundlessBentoVersion}-ubuntu-24.04-nvidia*`;
const boundlessAmi = aws.ec2.getAmi({
    mostRecent: true,
    owners: ["self", "968153779208"], // Self and Boundless AWS account
    filters: [
        {
            name: "name",
            values: [boundlessAmiName]
        }
    ]
});

const imageId = pulumi.output(boundlessAmi).apply(ami => ami.id);

// Alert topics
const boundlessAlertsTopicArn = baseConfig.get('SLACK_ALERTS_TOPIC_ARN');
const boundlessPagerdutyTopicArn = baseConfig.get('PAGERDUTY_ALERTS_TOPIC_ARN');
const alertsTopicArns = [boundlessAlertsTopicArn, boundlessPagerdutyTopicArn].filter(Boolean) as string[];

// Base configuration for all components
const baseComponentConfig: BaseComponentConfig = {
    stackName,
    environment,
    vpcId,
    privateSubnetIds: privSubNetIds,
    publicSubnetIds: pubSubNetIds,
};

// Create security components
const security = new SecurityComponent(baseComponentConfig);

// Create data services (RDS PostgreSQL and ElastiCache Redis)
const dataServices = new DataServicesComponent({
    ...baseComponentConfig,
    taskDBName,
    taskDBUsername,
    taskDBPassword,
    securityGroupId: security.securityGroup.id,
    rdsInstanceClass: config.get("rdsInstanceClass") || "db.t4g.micro",
    redisNodeType: config.get("redisNodeType") || "cache.t4g.micro",
});

// Create manager instance
const manager = new ManagerComponent({
    ...baseComponentConfig,
    imageId,
    instanceType: managerInstanceType,
    securityGroupId: security.securityGroup.id,
    iamInstanceProfileName: security.ec2Profile.name,
    taskDBName,
    taskDBUsername,
    taskDBPassword,
    ethRpcUrl,
    privateKey,
    orderStreamUrl,
    verifierAddress,
    boundlessMarketAddress,
    setVerifierAddress,
    collateralTokenAddress,
    chainId,
    alertsTopicArns: alertsTopicArns,
    rdsEndpoint: dataServices.rdsEndpoint,
    redisEndpoint: dataServices.redisEndpoint,
    s3BucketName: dataServices.s3BucketName,
    // Broker configuration
    mcyclePrice,
    peakProveKhz,
    minDeadline,
    lookbackBlocks,
    maxCollateral,
    maxFileSize,
    maxMcycleLimit,
    maxConcurrentProofs,
    balanceWarnThreshold,
    balanceErrorThreshold,
    collateralBalanceWarnThreshold,
    collateralBalanceErrorThreshold,
    priorityRequestorAddresses,
    denyRequestorAddresses,
    maxFetchRetries,
    allowClientAddresses,
    lockinPriorityGas,
});

// Create worker clusters
const workerCluster = new WorkerClusterComponent({
    ...baseComponentConfig,
    imageId,
    securityGroupId: security.securityGroup.id,
    iamInstanceProfileName: security.ec2Profile.name,
    managerIp: manager.instance.privateIp,
    taskDBName,
    taskDBUsername,
    taskDBPassword,
    proverCount,
    executionCount,
    auxCount,
    alertsTopicArns: alertsTopicArns,
    rdsEndpoint: dataServices.rdsEndpoint,
    redisEndpoint: dataServices.redisEndpoint,
    s3BucketName: dataServices.s3BucketName,
});

// Create API Gateway with NLB
const apiGateway = new ApiGatewayComponent({
    ...baseComponentConfig,
    managerPrivateIp: manager.instance.privateIp,
    securityGroupId: security.securityGroup.id,
    apiKey: apiKey.apply(key => key),
});

// Outputs
export const managerInstanceId = manager.instance.id;
export const managerPrivateIp = manager.instance.privateIp;
export const managerPublicIp = manager.instance.publicIp;

// AMI information
export const amiId = imageId;
export const amiName = pulumi.output(boundlessAmi).apply(ami => ami.name);
export const amiDescription = pulumi.output(boundlessAmi).apply(ami => ami.description);
export const boundlessVersionUsed = boundlessBentoVersion;

// ASG outputs
export const proverAsgName = workerCluster.proverAsg.autoScalingGroup.name;
export const proverAsgArn = workerCluster.proverAsg.autoScalingGroup.arn;
export const proverDesiredCapacity = workerCluster.proverAsg.autoScalingGroup.desiredCapacity;
export const proverMinSize = workerCluster.proverAsg.autoScalingGroup.minSize;
export const proverMaxSize = workerCluster.proverAsg.autoScalingGroup.maxSize;

// Execution ASG outputs
export const executionAsgName = workerCluster.executionAsg.autoScalingGroup.name;
export const executionAsgArn = workerCluster.executionAsg.autoScalingGroup.arn;
export const executionDesiredCapacity = workerCluster.executionAsg.autoScalingGroup.desiredCapacity;
export const executionMinSize = workerCluster.executionAsg.autoScalingGroup.minSize;
export const executionMaxSize = workerCluster.executionAsg.autoScalingGroup.maxSize;

// Aux ASG outputs
export const auxAsgName = workerCluster.auxAsg.autoScalingGroup.name;
export const auxAsgArn = workerCluster.auxAsg.autoScalingGroup.arn;
export const auxDesiredCapacity = workerCluster.auxAsg.autoScalingGroup.desiredCapacity;
export const auxMinSize = workerCluster.auxAsg.autoScalingGroup.minSize;
export const auxMaxSize = workerCluster.auxAsg.autoScalingGroup.maxSize;

// Data services outputs
export const rdsEndpoint = dataServices.rdsEndpoint;
export const redisEndpoint = dataServices.redisEndpoint;
export const s3BucketName = dataServices.s3BucketName;

// Shared credentials for prover nodes
export const sharedCredentials = pulumi.all([dataServices.rdsEndpoint, dataServices.redisEndpoint, dataServices.s3BucketName]).apply(([rdsEp, redisEp, s3Bucket]) => {
    const rdsHost = rdsEp.split(':')[0];
    const redisHost = redisEp.split(':')[0];
    return {
        postgresHost: rdsHost,
        postgresPort: "5432",
        postgresDb: taskDBName,
        postgresUser: taskDBUsername,
        postgresPassword: taskDBPassword,
        redisHost: redisHost,
        redisPort: "6379",
        s3Bucket: s3Bucket,
        s3Region: "us-west-2",
    };
});

// ALB outputs
export const albUrl = apiGateway.albUrl;
export const albDnsName = apiGateway.alb.dnsName;
export const targetGroupArn = apiGateway.targetGroup.arn;

// Cluster info
export const clusterInfo = {
    manager: {
        instanceId: manager.instance.id,
        publicIp: manager.instance.publicIp,
        privateIp: manager.instance.privateIp,
    },
    proverAsg: {
        name: workerCluster.proverAsg.autoScalingGroup.name,
        arn: workerCluster.proverAsg.autoScalingGroup.arn,
        desiredCapacity: workerCluster.proverAsg.autoScalingGroup.desiredCapacity,
        minSize: workerCluster.proverAsg.autoScalingGroup.minSize,
        maxSize: workerCluster.proverAsg.autoScalingGroup.maxSize,
        instanceType: "g6.xlarge",
    },
    ami: {
        id: imageId,
        name: pulumi.output(boundlessAmi).apply(ami => ami.name),
        version: boundlessBentoVersion,
    },
    apiGateway: {
        albUrl: apiGateway.albUrl,
        albDnsName: apiGateway.alb.dnsName,
        targetGroupArn: apiGateway.targetGroup.arn,
    },
};
