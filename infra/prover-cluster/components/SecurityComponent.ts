import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface SecurityComponentConfig extends BaseComponentConfig { }

export class SecurityComponent extends BaseComponent {
    public readonly ec2Role: aws.iam.Role;
    public readonly ec2Profile: aws.iam.InstanceProfile;
    public readonly securityGroup: aws.ec2.SecurityGroup;
    public readonly s3User: aws.iam.User;
    public readonly s3AccessKey: aws.iam.AccessKey;
    public readonly s3AccessKeyId: pulumi.Output<string>;
    public readonly s3SecretAccessKey: pulumi.Output<string>;

    constructor(config: SecurityComponentConfig) {
        super(config, "boundless-bento");
        this.ec2Role = this.createEC2Role();
        this.ec2Profile = this.createInstanceProfile();
        this.securityGroup = this.createSecurityGroup();
        const s3UserResources = this.createS3User();
        this.s3User = s3UserResources.user;
        this.s3AccessKey = s3UserResources.accessKey;
        this.s3AccessKeyId = s3UserResources.accessKeyId;
        this.s3SecretAccessKey = s3UserResources.secretAccessKey;
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

        // Attach S3 access policy - scoped to the bento-storage bucket
        const bucketName = this.generateName("bento-storage");
        new aws.iam.RolePolicy("ec2-s3-scoped-policy", {
            role: role.id,
            policy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "s3:CreateBucket",
                            "s3:ListBucket",
                            "s3:GetBucketLocation"
                        ],
                        Resource: `arn:aws:s3:::${bucketName}`
                    },
                    {
                        Effect: "Allow",
                        Action: [
                            "s3:*"
                        ],
                        Resource: `arn:aws:s3:::${bucketName}/*`
                    }
                ]
            })
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
                {
                    protocol: "tcp",
                    fromPort: 6379,
                    toPort: 6379,
                    self: true,
                    description: "Redis/Valkey access from cluster instances"
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

    private createS3User(): {
        user: aws.iam.User;
        accessKey: aws.iam.AccessKey;
        accessKeyId: pulumi.Output<string>;
        secretAccessKey: pulumi.Output<string>;
    } {
        const user = new aws.iam.User("s3User", {
            name: this.generateName("s3-user"),
            path: "/boundless/",
        });

        // Attach S3 access policy - scoped to the bento-storage bucket
        const bucketName = this.generateName("bento-storage");
        new aws.iam.UserPolicy("s3UserScopedPolicy", {
            user: user.name,
            policy: JSON.stringify({
                Version: "2012-10-17",
                Statement: [
                    {
                        Effect: "Allow",
                        Action: [
                            "s3:CreateBucket",
                            "s3:ListBucket",
                            "s3:GetBucketLocation"
                        ],
                        Resource: `arn:aws:s3:::${bucketName}`
                    },
                    {
                        Effect: "Allow",
                        Action: [
                            "s3:*"
                        ],
                        Resource: `arn:aws:s3:::${bucketName}/*`
                    }
                ]
            })
        });

        // Create access key
        const accessKey = new aws.iam.AccessKey("s3AccessKey", {
            user: user.name,
        });

        return {
            user,
            accessKey,
            accessKeyId: accessKey.id,
            secretAccessKey: accessKey.secret,
        };
    }
}
