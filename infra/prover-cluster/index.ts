import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import {
    SecurityComponent,
    ManagerComponent,
    WorkerClusterComponent,
    ApiGatewayComponent,
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

// Contract addresses
const taskDBUsername: string = config.require("taskDBUsername");
const taskDBPassword: string = config.require("taskDBPassword");
const taskDBName: string = config.require("taskDBName");

// MinIO configuration
const minioUsername: string = config.get("minioUsername") || "minioadmin";
const minioPassword: string = config.get("minioPassword") || "minioadmin123";

// Worker counts
const executionCount: number = config.getNumber("executionCount") || 1;
const proverCount: number = config.getNumber("proverWorkerCount") || 1;
const auxCount: number = config.getNumber("auxWorkerCount") || 1;

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

// Create manager (single instance) cluster
const manager = new ManagerComponent({
    ...baseComponentConfig,
    imageId,
    instanceType: managerInstanceType,
    securityGroupId: security.securityGroup.id,
    iamInstanceProfileName: security.ec2Profile.name,
    taskDBName,
    taskDBUsername,
    taskDBPassword,
    minioUsername,
    minioPassword,
    ethRpcUrl,
    privateKey,
    orderStreamUrl,
    verifierAddress,
    boundlessMarketAddress,
    setVerifierAddress,
    collateralTokenAddress,
    chainId,
    alertsTopicArns: alertsTopicArns,
});

// Create worker clusters
const workerCluster = new WorkerClusterComponent({
    ...baseComponentConfig,
    imageId,
    securityGroupId: security.securityGroup.id,
    iamInstanceProfileName: security.ec2Profile.name,
    managerIp: manager.managerNetworkInterface.privateIp,
    taskDBName,
    taskDBUsername,
    taskDBPassword,
    minioUsername,
    minioPassword,
    proverCount,
    executionCount,
    auxCount,
    alertsTopicArns: alertsTopicArns,
});

// Create API Gateway with NLB
const apiGateway = new ApiGatewayComponent({
    ...baseComponentConfig,
    managerPrivateIp: manager.managerNetworkInterface.privateIp,
    securityGroupId: security.securityGroup.id,
    apiKey: apiKey.apply(key => key),
});

// Outputs
export const managerPrivateIp = manager.managerNetworkInterface.privateIp;
export const managerAsgName = manager.managerAsg.autoScalingGroup.name;
export const managerAsgArn = manager.managerAsg.autoScalingGroup.arn;
export const managerDesiredCapacity = manager.managerAsg.autoScalingGroup.desiredCapacity;
export const managerMinSize = manager.managerAsg.autoScalingGroup.minSize;
export const managerMaxSize = manager.managerAsg.autoScalingGroup.maxSize;

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

// Redis connection details
export const redisHost = manager.managerNetworkInterface.privateIp;
export const redisPort = "6379";

// Shared credentials for prover nodes
export const sharedCredentials = {
    postgresHost: manager.managerNetworkInterface.privateIp,
    postgresPort: "5432",
    postgresDb: taskDBName,
    postgresUser: taskDBUsername,
    postgresPassword: taskDBPassword,
    redisHost: manager.managerNetworkInterface.privateIp,
    redisPort: "6379",
    s3Bucket: "bento",
    s3Region: "us-west-2",
};

// ALB outputs
export const albUrl = apiGateway.albUrl;
export const albDnsName = apiGateway.alb.dnsName;
export const targetGroupArn = apiGateway.targetGroup.arn;

// Cluster info
export const clusterInfo = {
    manager: {
        name: manager.managerAsg.autoScalingGroup.name,
        arn: manager.managerAsg.autoScalingGroup.arn,
        desiredCapacity: manager.managerAsg.autoScalingGroup.desiredCapacity,
        minSize: manager.managerAsg.autoScalingGroup.minSize,
        maxSize: manager.managerAsg.autoScalingGroup.maxSize,
        privateIp: manager.managerNetworkInterface.privateIp,
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
