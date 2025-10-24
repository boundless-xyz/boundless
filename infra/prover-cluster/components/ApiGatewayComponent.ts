import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface ApiGatewayComponentConfig extends BaseComponentConfig {
    managerPrivateIp: pulumi.Output<string>;
    securityGroupId: pulumi.Output<string>;
    apiKey: pulumi.Output<string>;
}

export class ApiGatewayComponent extends BaseComponent {
    public readonly alb: aws.lb.LoadBalancer;
    public readonly targetGroup: aws.lb.TargetGroup;
    public readonly albUrl: pulumi.Output<string>;
    public readonly wafWebAcl: aws.wafv2.WebAcl;
    private readonly apiKey: pulumi.Output<string>;

    constructor(config: ApiGatewayComponentConfig) {
        super(config, "api-gateway");
        this.apiKey = config.apiKey;

        // Create public Application Load Balancer
        this.alb = new aws.lb.LoadBalancer("boundless-alb", {
            name: `${this.config.stackName}-boundless-alb`,
            internal: false, // Public ALB
            loadBalancerType: "application",
            subnets: config.publicSubnetIds, // Use public subnets
            enableDeletionProtection: false,
            securityGroups: [config.securityGroupId],
            tags: {
                Name: `${this.config.stackName}-boundless-alb`,
                Environment: this.config.stackName,
                Component: "api-gateway"
            }
        });

        // Create target group for the manager instance
        this.targetGroup = new aws.lb.TargetGroup("boundless-tg", {
            name: `${this.config.stackName}-boundless-tg`,
            port: 8081,
            protocol: "HTTP",
            vpcId: config.vpcId,
            targetType: "ip",
            healthCheck: {
                enabled: true,
                healthyThreshold: 2,
                interval: 30,
                port: "traffic-port",
                protocol: "HTTP",
                path: "/health",
                timeout: 5,
                unhealthyThreshold: 2,
            },
            tags: {
                Name: `${this.config.stackName}-boundless-tg`,
                Environment: this.config.stackName,
                Component: "api-gateway"
            }
        });

        // Register the manager instance as a target
        new aws.lb.TargetGroupAttachment("boundless-tg-attachment", {
            targetGroupArn: this.targetGroup.arn,
            targetId: config.managerPrivateIp,
            port: 8081,
        });

        // Create ALB listener for HTTP (port 80)
        new aws.lb.Listener("boundless-alb-listener", {
            loadBalancerArn: this.alb.arn,
            port: 80,
            protocol: "HTTP",
            defaultActions: [{
                type: "forward",
                targetGroupArn: this.targetGroup.arn,
            }],
        });

        // Create WAF Web ACL with API key enforcement
        this.wafWebAcl = new aws.wafv2.WebAcl("boundless-waf", {
            name: `${this.config.stackName}-boundless-waf`,
            description: "WAF for Boundless Bento API with API key enforcement",
            scope: "REGIONAL",
            defaultAction: {
                block: {}
            },
            rules: [
                {
                    name: "ApiKeyRule",
                    priority: 1,
                    action: {
                        allow: {}
                    },
                    statement: {
                        byteMatchStatement: {
                            searchString: this.apiKey,
                            fieldToMatch: {
                                singleHeader: {
                                    name: "x-api-key"
                                }
                            },
                            textTransformations: [
                                {
                                    priority: 0,
                                    type: "NONE"
                                }
                            ],
                            positionalConstraint: "EXACTLY"
                        }
                    },
                    visibilityConfig: {
                        cloudwatchMetricsEnabled: true,
                        metricName: "ApiKeyRule",
                        sampledRequestsEnabled: true
                    }
                },
                {
                    name: "HealthCheckRule",
                    priority: 2,
                    action: {
                        allow: {}
                    },
                    statement: {
                        byteMatchStatement: {
                            searchString: "/health",
                            fieldToMatch: {
                                uriPath: {}
                            },
                            textTransformations: [
                                {
                                    priority: 0,
                                    type: "LOWERCASE"
                                }
                            ],
                            positionalConstraint: "STARTS_WITH"
                        }
                    },
                    visibilityConfig: {
                        cloudwatchMetricsEnabled: true,
                        metricName: "HealthCheckRule",
                        sampledRequestsEnabled: true
                    }
                }
            ],
            visibilityConfig: {
                cloudwatchMetricsEnabled: true,
                metricName: "BoundlessWaf",
                sampledRequestsEnabled: true
            },
            tags: {
                Name: `${this.config.stackName}-boundless-waf`,
                Environment: this.config.stackName,
                Component: "api-gateway"
            }
        });

        // Associate WAF with ALB
        new aws.wafv2.WebAclAssociation("boundless-waf-association", {
            resourceArn: this.alb.arn,
            webAclArn: this.wafWebAcl.arn
        });

        // Set the ALB URL for external access
        this.albUrl = this.alb.dnsName.apply(dnsName => `http://${dnsName}`);
    }
}
