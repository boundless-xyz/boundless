import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as crypto from "crypto";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface AutoScalingGroupConfig extends BaseComponentConfig {
    launchTemplateId: pulumi.Output<string>;
    launchTemplateUserData: pulumi.Output<string>;
    minSize: number;
    maxSize: number;
    desiredCapacity: number;
    componentType: "prover" | "execution" | "aux";
}

export class AutoScalingGroupComponent extends BaseComponent {
    public readonly autoScalingGroup: aws.autoscaling.Group;

    constructor(config: AutoScalingGroupConfig) {
        super(config, "boundless-bento");
        this.autoScalingGroup = this.createAutoScalingGroup(config);
    }

    private createAutoScalingGroup(config: AutoScalingGroupConfig): aws.autoscaling.Group {
        return new aws.autoscaling.Group(`${config.componentType}-asg`, {
            name: this.generateName(`${config.componentType}-asg`),
            vpcZoneIdentifiers: this.config.privateSubnetIds,
            minSize: config.minSize,
            maxSize: config.maxSize,
            desiredCapacity: config.desiredCapacity,
            launchTemplate: {
                id: config.launchTemplateId,
                version: "$Latest",
            },
            healthCheckType: "EC2",
            healthCheckGracePeriod: 300,
            defaultCooldown: 300,
            terminationPolicies: ["OldestInstance"],
            enabledMetrics: ["GroupInServiceInstances"],
            tags: [
                {
                    key: "userDataHash",
                    value: config.launchTemplateUserData.apply(u =>
                        crypto.createHash("sha256").update(`${u || ""}-${Date.now()}`).digest("hex")
                    ),
                    propagateAtLaunch: true,
                },
                {
                    key: "Name",
                    value: this.generateName(`${config.componentType}`),
                    propagateAtLaunch: false,
                },
                {
                    key: "Type",
                    value: `${config.componentType}-asg`,
                    propagateAtLaunch: false,
                },
                {
                    key: "Environment",
                    value: this.config.environment,
                    propagateAtLaunch: true,
                },
                {
                    key: "Project",
                    value: "boundless-bento-cluster",
                    propagateAtLaunch: true,
                },
            ],
            instanceRefresh: {
                strategy: "Rolling",
                preferences: {
                    minHealthyPercentage: 0,
                },
                triggers: ["tag"],
            },
        });
    }
}
