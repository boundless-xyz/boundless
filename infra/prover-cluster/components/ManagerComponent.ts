import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";
import { LaunchTemplateComponent, LaunchTemplateConfig } from "./LaunchTemplateComponent";
import { ManagerMetricAlarmComponent } from "./MetricAlarmComponent";

export interface ManagerComponentConfig extends BaseComponentConfig {
    imageId: pulumi.Output<string>;
    instanceType: string;
    securityGroupId: pulumi.Output<string>;
    iamInstanceProfileName: pulumi.Output<string>;
    taskDBName: string;
    taskDBUsername: string;
    taskDBPassword: string;
    ethRpcUrl: pulumi.Output<string>;
    privateKey: pulumi.Output<string>;
    orderStreamUrl: string;
    verifierAddress: string;
    boundlessMarketAddress: string;
    setVerifierAddress: string;
    collateralTokenAddress: string;
    chainId: string;
    alertsTopicArns: string[];
    rdsEndpoint: pulumi.Output<string>;
    redisEndpoint: pulumi.Output<string>;
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
    balanceWarnThreshold: string;
    balanceErrorThreshold: string;
    collateralBalanceWarnThreshold: string;
    collateralBalanceErrorThreshold: string;
    priorityRequestorAddresses: string;
    denyRequestorAddresses: string;
    maxFetchRetries: number;
    allowClientAddresses: string;
    lockinPriorityGas: string;
}

export class ManagerComponent extends BaseComponent {
    public readonly instance: aws.ec2.Instance;
    public readonly launchTemplate: LaunchTemplateComponent;
    public readonly metricAlarms: ManagerMetricAlarmComponent;

    constructor(config: ManagerComponentConfig) {
        super(config, "boundless-bento");

        const launchTemplateConfig: LaunchTemplateConfig = {
            ...config,
            componentType: "manager",
            volumeSize: 1024,
        };

        this.launchTemplate = new LaunchTemplateComponent(launchTemplateConfig);
        this.instance = this.createManagerInstance(config);
        this.metricAlarms = this.createManagerAlarms(config);
    }

    private createManagerInstance(config: ManagerComponentConfig): aws.ec2.Instance {
        // Use the launch template's latest version number to force replacement when userdata changes
        // When the launch template is updated, latestVersion changes, which will replace the instance
        const launchTemplateVersion = pulumi.output(this.launchTemplate.launchTemplate.latestVersion).apply(v =>
            v ? String(v) : "$Latest"
        );

        return new aws.ec2.Instance("manager", {
            // Use launch template instead of duplicating configuration
            // Use the specific version number instead of "$Latest" to force replacement when version changes
            launchTemplate: pulumi.all([this.launchTemplate.launchTemplate.id, launchTemplateVersion]).apply(([id, version]) => ({
                id: id,
                version: version,
            })),
            subnetId: this.config.privateSubnetIds.apply((subnets: string[]) => subnets[0]),
            ebsBlockDevices: [{
                deviceName: "/dev/sda1",
                volumeSize: 1024,
                volumeType: "gp3",
                deleteOnTermination: true,
            }],
            tags: {
                Name: `${this.config.stackName}-manager`,
                Type: "manager",
                Environment: this.config.environment,
                Project: `${this.config.stackName}-prover`,
                "ssm:bootstrap": "manager",
            },
        });
    }

    private createManagerAlarms(config: ManagerComponentConfig): ManagerMetricAlarmComponent {
        return new ManagerMetricAlarmComponent({
            ...config,
            serviceName: "bento-manager",
            logGroupName: `/boundless/bento/${config.stackName}/manager`,
            alarmDimensions: { InstanceId: this.instance.id },
        });
    }
}
