import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";
import { LaunchTemplateComponent, LaunchTemplateConfig } from "./LaunchTemplateComponent";

export interface ManagerComponentConfig extends BaseComponentConfig {
    imageId: pulumi.Output<string>;
    instanceType: string;
    securityGroupId: pulumi.Output<string>;
    iamInstanceProfileName: pulumi.Output<string>;
    taskDBName: string;
    taskDBUsername: string;
    taskDBPassword: string;
    minioUsername: string;
    minioPassword: string;
    ethRpcUrl: pulumi.Output<string>;
    privateKey: pulumi.Output<string>;
    orderStreamUrl: string;
    verifierAddress: string;
    boundlessMarketAddress: string;
    setVerifierAddress: string;
    collateralTokenAddress: string;
    chainId: string;
}

export class ManagerComponent extends BaseComponent {
    public readonly instance: aws.ec2.Instance;
    public readonly launchTemplate: LaunchTemplateComponent;

    constructor(config: ManagerComponentConfig) {
        super(config, "boundless-bento");

        const launchTemplateConfig: LaunchTemplateConfig = {
            ...config,
            componentType: "manager",
            volumeSize: 1024,
        };

        this.launchTemplate = new LaunchTemplateComponent(launchTemplateConfig);
        this.instance = this.createManagerInstance(config);
    }

    private createManagerInstance(config: ManagerComponentConfig): aws.ec2.Instance {
        return new aws.ec2.Instance("manager", {
            ami: config.imageId,
            instanceType: config.instanceType,
            subnetId: this.config.privateSubnetIds.apply((subnets: string[]) => subnets[0]),
            vpcSecurityGroupIds: [config.securityGroupId],
            iamInstanceProfile: config.iamInstanceProfileName,
            userData: pulumi.output(this.launchTemplate.launchTemplate.userData).apply(u => u || ""),
            userDataReplaceOnChange: false,
            ebsBlockDevices: [{
                deviceName: "/dev/sda1",
                volumeSize: 1024,
                volumeType: "gp3",
                deleteOnTermination: true,
            }],
            tags: {
                Name: "boundless-bento-manager",
                Type: "manager",
                Environment: this.config.environment,
                Project: "boundless-bento-cluster",
                "ssm:bootstrap": "manager",
            },
        });
    }
}
