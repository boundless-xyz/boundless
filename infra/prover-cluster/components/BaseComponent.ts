import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export interface BaseComponentConfig {
    stackName: string;
    environment: string;
    vpcId: pulumi.Output<string>;
    privateSubnetIds: pulumi.Output<string[]>;
    publicSubnetIds: pulumi.Output<string[]>;
}

export abstract class BaseComponent {
    protected config: BaseComponentConfig;
    protected namePrefix: string;

    constructor(config: BaseComponentConfig, namePrefix: string) {
        this.config = config;
        this.namePrefix = namePrefix;
    }

    protected generateName(suffix: string): string {
        return `${this.namePrefix}-${suffix}-${this.config.stackName}`;
    }

    protected generateTagName(suffix: string): string {
        return `boundless-bento-${suffix}`;
    }

    protected getCommonTags(componentType: string, instanceType?: string): Record<string, string> {
        const tags: Record<string, string> = {
            Environment: this.config.environment,
            Project: "boundless-bento-cluster",
            Component: componentType,
        };

        if (instanceType) {
            tags.InstanceType = instanceType;
        }

        return tags;
    }
}
