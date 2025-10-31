import * as pulumi from "@pulumi/pulumi";
import {BaseComponent, BaseComponentConfig} from "./BaseComponent";
import {LaunchTemplateComponent, LaunchTemplateConfig} from "./LaunchTemplateComponent";
import {AutoScalingGroupComponent, AutoScalingGroupConfig} from "./AutoScalingGroupComponent";
import {ProverMetricAlarmComponent, WorkerClusterAlarmComponent} from "./MetricAlarmComponent";

export interface WorkerClusterConfig extends BaseComponentConfig {
    imageId: pulumi.Output<string>;
    securityGroupId: pulumi.Output<string>;
    iamInstanceProfileName: pulumi.Output<string>;
    managerIp: pulumi.Output<string>;
    taskDBName: string;
    taskDBUsername: string;
    taskDBPassword: string;
    minioUsername: string;
    minioPassword: string;
    proverCount: number;
    executionCount: number;
    auxCount: number;
    alertsTopicArns: string[];
}

export class WorkerClusterComponent extends BaseComponent {
    public readonly proverAsg: AutoScalingGroupComponent;
    public readonly executionAsg: AutoScalingGroupComponent;
    public readonly auxAsg: AutoScalingGroupComponent;
    public readonly proverAlarms: ProverMetricAlarmComponent;
    public readonly executionAlarms: WorkerClusterAlarmComponent;
    public readonly auxAlarms: WorkerClusterAlarmComponent;

    constructor(config: WorkerClusterConfig) {
        super(config, "boundless-bento");

        // Create prover cluster
        this.proverAsg = this.createProverCluster(config);

        // Create execution cluster
        this.executionAsg = this.createExecutionCluster(config);

        // Create aux cluster
        this.auxAsg = this.createAuxCluster(config);

        // Create prover cluster alarms
        this.proverAlarms = this.createProverAlarms(config);

        // Create execution cluster alarms
        this.executionAlarms = this.createExecutionAlarms(config);

        // Create aux cluster alarms
        this.auxAlarms = this.createAuxAlarms(config);
    }

    private createProverCluster(config: WorkerClusterConfig): AutoScalingGroupComponent {
        const launchTemplateConfig: LaunchTemplateConfig = {
            ...config,
            instanceType: "g6.xlarge",
            componentType: "prover",
            volumeSize: 100,
        };

        const launchTemplate = new LaunchTemplateComponent(launchTemplateConfig);

        const asgConfig: AutoScalingGroupConfig = {
            ...config,
            launchTemplateId: launchTemplate.launchTemplate.id,
            launchTemplateUserData: pulumi.output(launchTemplate.launchTemplate.userData).apply(u => u || ""),
            minSize: config.proverCount,
            maxSize: config.proverCount,
            desiredCapacity: config.proverCount,
            componentType: "prover",
        };

        return new AutoScalingGroupComponent(asgConfig);
    }

    private createProverAlarms(config: WorkerClusterConfig): ProverMetricAlarmComponent {
        return new ProverMetricAlarmComponent({
            ...config,
            serviceName: "bento-prover-cluster",
            logGroupName: `/boundless/bento/${config.stackName}/prover`,
            alarmDimensions: {AutoScalingGroupName: this.proverAsg.autoScalingGroup.name}
        });
    }

    private createExecutionCluster(config: WorkerClusterConfig): AutoScalingGroupComponent {
        const launchTemplateConfig: LaunchTemplateConfig = {
            ...config,
            instanceType: "c7i.large",
            componentType: "execution",
            volumeSize: 100,
        };

        const launchTemplate = new LaunchTemplateComponent(launchTemplateConfig);

        const asgConfig: AutoScalingGroupConfig = {
            ...config,
            launchTemplateId: launchTemplate.launchTemplate.id,
            launchTemplateUserData: pulumi.output(launchTemplate.launchTemplate.userData).apply(u => u || ""),
            minSize: config.executionCount,
            maxSize: config.executionCount,
            desiredCapacity: config.executionCount,
            componentType: "execution",
        };

        return new AutoScalingGroupComponent(asgConfig);
    }

    private createExecutionAlarms(config: WorkerClusterConfig): WorkerClusterAlarmComponent {
        return new WorkerClusterAlarmComponent({
            ...config,
            serviceName: "bento-execution-cluster",
            logGroupName: `/boundless/bento/${config.stackName}/execution`,
            alarmDimensions: {AutoScalingGroupName: this.executionAsg.autoScalingGroup.name}
        });
    }

    private createAuxCluster(config: WorkerClusterConfig): AutoScalingGroupComponent {
        const launchTemplateConfig: LaunchTemplateConfig = {
            ...config,
            instanceType: "t3.medium",
            componentType: "aux",
            volumeSize: 100,
        };

        const launchTemplate = new LaunchTemplateComponent(launchTemplateConfig);

        const asgConfig: AutoScalingGroupConfig = {
            ...config,
            launchTemplateId: launchTemplate.launchTemplate.id,
            launchTemplateUserData: pulumi.output(launchTemplate.launchTemplate.userData).apply(u => u || ""),
            minSize: config.auxCount,
            maxSize: config.auxCount,
            desiredCapacity: config.auxCount,
            componentType: "aux",
        };

        return new AutoScalingGroupComponent(asgConfig);
    }

    private createAuxAlarms(config: WorkerClusterConfig): WorkerClusterAlarmComponent {
        return new WorkerClusterAlarmComponent({
            ...config,
            serviceName: "bento-aux-cluster",
            logGroupName: `/boundless/bento/${config.stackName}/aux`,
            alarmDimensions: {AutoScalingGroupName: this.auxAsg.autoScalingGroup.name}
        });
    }
}
