import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface SecurityComponentConfig extends BaseComponentConfig { }

export class SecurityComponent extends BaseComponent {
    public readonly ec2Role: aws.iam.Role;
    public readonly ec2Profile: aws.iam.InstanceProfile;
    public readonly securityGroup: aws.ec2.SecurityGroup;

    constructor(config: SecurityComponentConfig) {
        super(config, "boundless-bento");
        this.ec2Role = this.createEC2Role();
        this.ec2Profile = this.createInstanceProfile();
        this.securityGroup = this.createSecurityGroup();
    }

    private createEC2Role(): aws.iam.Role {
        const role = new aws.iam.Role("ec2SsmRole", {
            name: this.generateName("ec2-ssm-role"),
            assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({ Service: "ec2.amazonaws.com" }),
        });

        // Attach SSM managed instance core policy
        new aws.iam.RolePolicyAttachment("attachSsmCore", {
            role: role.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
        });

        // Attach CloudWatch policy
        new aws.iam.RolePolicyAttachment("ec2-cloudwatch-policy", {
            role: role.name,
            policyArn: "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
        });

        // Custom policy for Vector logs
        new aws.iam.RolePolicy("ec2-vector-logs-policy", {
            role: role.id,
            policy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "logs:CreateLogGroup",
                            "logs:CreateLogStream",
                            "logs:DescribeLogGroups",
                            "logs:DescribeLogStreams",
                            "logs:PutLogEvents"
                        ],
                        Resource: "*"
                    }
                ]
            })
        });

        // Attach S3 access policy
        new aws.iam.RolePolicyAttachment("ec2-s3-policy", {
            role: role.name,
            policyArn: "arn:aws:iam::aws:policy/AmazonS3FullAccess",
        });

        return role;
    }

    private createInstanceProfile(): aws.iam.InstanceProfile {
        return new aws.iam.InstanceProfile("ec2Profile", {
            name: this.generateName("ec2-profile"),
            role: this.ec2Role.name
        });
    }

    private createSecurityGroup(): aws.ec2.SecurityGroup {
        return new aws.ec2.SecurityGroup("manager-sg", {
            vpcId: this.config.vpcId,
            description: "Security group for boundless bento cluster",
            ingress: [
                {
                    protocol: "tcp",
                    fromPort: 22,
                    toPort: 22,
                    cidrBlocks: ["0.0.0.0/0"],
                    description: "SSH access"
                },
                {
                    protocol: "tcp",
                    fromPort: 80,
                    toPort: 80,
                    cidrBlocks: ["0.0.0.0/0"],
                    description: "HTTP access for ALB"
                },
                {
                    protocol: "tcp",
                    fromPort: 8081,
                    toPort: 8081,
                    cidrBlocks: ["0.0.0.0/0"],
                    description: "Bento API access"
                },
                {
                    protocol: "tcp",
                    fromPort: 8081,
                    toPort: 8081,
                    cidrBlocks: ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"],
                    description: "VPC Link access to Bento API"
                },
                {
                    protocol: "tcp",
                    fromPort: 8081,
                    toPort: 8081,
                    cidrBlocks: ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"],
                    description: "VPC Link access to Bento API"
                },
            ],
            egress: [
                {
                    protocol: "-1",
                    fromPort: 0,
                    toPort: 0,
                    cidrBlocks: ["0.0.0.0/0"],
                    description: "All outbound traffic"
                }
            ],
        });
    }
}
