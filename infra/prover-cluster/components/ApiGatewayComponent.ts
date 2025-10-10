import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface ApiGatewayComponentConfig extends BaseComponentConfig {
    managerPrivateIp: pulumi.Output<string>;
    securityGroupId: pulumi.Output<string>;
}

export class ApiGatewayComponent extends BaseComponent {
    public readonly api: aws.apigateway.RestApi;
    public readonly apiKey: aws.apigateway.ApiKey;
    public readonly stage: aws.apigateway.Stage;
    public readonly nlb: aws.lb.LoadBalancer;

    constructor(config: ApiGatewayComponentConfig) {
        super(config, "api-gateway");

        // Create simple REST API
        this.api = new aws.apigateway.RestApi("boundless-api", {
            name: `${this.config.stackName}-boundless-api`,
            description: "Boundless Bento API",
            endpointConfiguration: { types: "REGIONAL" },
        });

        // Create API Key
        this.apiKey = new aws.apigateway.ApiKey("boundless-api-key", {
            name: `${this.config.stackName}-boundless-api-key`,
            enabled: true,
        });

        // Create proxy resource
        const proxy = new aws.apigateway.Resource("proxy", {
            restApi: this.api.id,
            parentId: this.api.rootResourceId,
            pathPart: "{proxy+}",
        });

        // Create ANY method with API key requirement
        const method = new aws.apigateway.Method("proxy-method", {
            restApi: this.api.id,
            resourceId: proxy.id,
            httpMethod: "ANY",
            authorization: "NONE",
            apiKeyRequired: true,
            requestParameters: {
                "method.request.path.proxy": true
            },
        });

        // Create Network Load Balancer
        this.nlb = new aws.lb.LoadBalancer("boundless-nlb", {
            name: `${this.config.stackName}-boundless-nlb`,
            internal: true,
            loadBalancerType: "network",
            subnets: config.privateSubnetIds,
            enableDeletionProtection: false,
        });

        // Create target group for the manager instance
        const targetGroup = new aws.lb.TargetGroup("boundless-tg-v2", {
            name: `${this.config.stackName}-boundless-tg-v2`,
            port: 8081,
            protocol: "TCP",
            vpcId: config.vpcId,
            targetType: "ip",
            healthCheck: {
                enabled: true,
                healthyThreshold: 2,
                interval: 30,
                port: "traffic-port",
                protocol: "TCP",
                timeout: 5,
                unhealthyThreshold: 2,
            },
        });

        // Register the manager instance as a target
        new aws.lb.TargetGroupAttachment("boundless-tg-attachment-v2", {
            targetGroupArn: targetGroup.arn,
            targetId: config.managerPrivateIp,
            port: 8081,
        });

        // Create NLB listener
        new aws.lb.Listener("boundless-nlb-listener", {
            loadBalancerArn: this.nlb.arn,
            port: 8081,
            protocol: "TCP",
            defaultActions: [{
                type: "forward",
                targetGroupArn: targetGroup.arn,
            }],
        });

        // Create VPC Link
        const vpcLink = new aws.apigateway.VpcLink("boundless-vpc-link", {
            name: `${this.config.stackName}-boundless-vpc-link`,
            targetArn: this.nlb.arn,
        });

        // Create integration using VPC Link
        const integration = new aws.apigateway.Integration("proxy-integration", {
            restApi: this.api.id,
            resourceId: proxy.id,
            httpMethod: method.httpMethod,
            integrationHttpMethod: "ANY",
            type: "HTTP_PROXY",
            uri: pulumi.interpolate`http://${this.nlb.dnsName}:8081/{proxy}`,
            connectionType: "VPC_LINK",
            connectionId: vpcLink.id,
            passthroughBehavior: "WHEN_NO_MATCH",
            requestParameters: {
                "integration.request.path.proxy": "method.request.path.proxy"
            },
        });

        // Create deployment
        const deployment = new aws.apigateway.Deployment("boundless-deployment", {
            restApi: this.api.id,
        }, {
            dependsOn: [integration],
        });

        // Create stage
        this.stage = new aws.apigateway.Stage("boundless-stage", {
            deployment: deployment.id,
            restApi: this.api.id,
            stageName: "prod",
        });

        // Create usage plan and link API key
        const usagePlan = new aws.apigateway.UsagePlan("boundless-usage-plan", {
            apiStages: [{ apiId: this.api.id, stage: this.stage.stageName }],
        });

        new aws.apigateway.UsagePlanKey("boundless-usage-plan-key", {
            keyId: this.apiKey.id,
            keyType: "API_KEY",
            usagePlanId: usagePlan.id,
        });
    }
}
