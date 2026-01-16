import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";
import { LaunchTemplateComponent, LaunchTemplateConfig } from "./LaunchTemplateComponent";
import { ManagerMetricAlarmComponent } from "./MetricAlarmComponent";
import { AutoScalingGroupComponent, AutoScalingGroupConfig } from "./AutoScalingGroupComponent";

export interface ManagerComponentConfig extends BaseComponentConfig {
    imageId: pulumi.Output<string>;
    instanceType: string;
    securityGroupId: pulumi.Output<string>;
    iamInstanceProfileName: pulumi.Output<string>;
    taskDBName: string;
    taskDBUsername: string;
    taskDBPassword: string;
    brokerRpcUrls: pulumi.Output<string>;
    privateKey: pulumi.Output<string>;
    orderStreamUrl: pulumi.Output<string>;
    verifierAddress: string;
    boundlessMarketAddress: string;
    setVerifierAddress: string;
    collateralTokenAddress: string;
    chainId: string;
    alertsTopicArns: string[];
    rdsEndpoint: pulumi.Output<string>;
    s3BucketName: pulumi.Output<string>;
    s3AccessKeyId: pulumi.Output<string>;
    s3SecretAccessKey: pulumi.Output<string>;
    // Broker configuration
    mcyclePrice: string;
    peakProveKhz: number;
    minDeadline: number;
    lookbackBlocks: number;
    maxCollateral: string;
    maxFileSize: string;
    maxMcycleLimit: string;
    maxConcurrentProofs: number;
    maxConcurrentPreflights: number;
    maxJournalBytes: number;
    balanceWarnThreshold: string;
    balanceErrorThreshold: string;
    collateralBalanceWarnThreshold: string;
    collateralBalanceErrorThreshold: string;
    priorityRequestorAddresses: string;
    denyRequestorAddresses: string;
    maxFetchRetries: number;
    allowRequestorLists: string;
    lockinPriorityGas: string;
    orderCommitmentPriority: string;
    rustLogLevel: string;
}

export class ManagerComponent extends BaseComponent {
    public readonly managerNetworkInterface: aws.ec2.NetworkInterface;
    public readonly managerAsg: AutoScalingGroupComponent;
    public readonly launchTemplate: LaunchTemplateComponent;
    public readonly metricAlarms: ManagerMetricAlarmComponent;

    constructor(config: ManagerComponentConfig) {
        super(config, "boundless-bento");

        // Network interface for the manager. This allows manager instances created and managed by
        // the manager ASG to have a static private IP that the cluster agents can refer to,
        // but limits the ASG instances to 1
        this.managerNetworkInterface = new aws.ec2.NetworkInterface(this.generateName("manager-network-interface"), {
            subnetId: config.privateSubnetIds[0],
            description: "Static network interface for boundless-bento manager instance",
            securityGroups: [config.securityGroupId],
            tags: {
                Name: this.generateTagName("manager"),
                Type: "manager",
                Environment: this.config.environment,
            }
        }
        )

        const launchTemplateConfig: LaunchTemplateConfig = {
            ...config,
            componentType: "manager",
            volumeSize: 1024,
            networkInterfaceId: this.managerNetworkInterface.id,
        };

        this.launchTemplate = new LaunchTemplateComponent(launchTemplateConfig);
        this.managerAsg = this.createManagerCluster(config);
        this.metricAlarms = this.createManagerAlarms(config);
    }

    private createManagerCluster(config: ManagerComponentConfig): AutoScalingGroupComponent {
        const asgConfig: AutoScalingGroupConfig = {
            ...config,
            launchTemplateId: this.launchTemplate.launchTemplate.id,
            launchTemplateUserData: pulumi.output(this.launchTemplate.launchTemplate.userData).apply(u => u || ""),
            // Manager ASG should be single-instance
            minSize: 1,
            maxSize: 1,
            desiredCapacity: 1,
            componentType: "manager",
        };

        return new AutoScalingGroupComponent(asgConfig);
    }

    private createManagerAlarms(config: ManagerComponentConfig): ManagerMetricAlarmComponent {
        return new ManagerMetricAlarmComponent({
            ...config,
            serviceName: "bento-manager",
            logGroupName: `/boundless/bento/${config.stackName}/manager`,
            alarmDimensions: { AutoScalingGroupName: this.managerAsg.autoScalingGroup.name },
            minAsgSize: this.managerAsg.autoScalingGroup.minSize
        });
    }
}
