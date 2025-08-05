import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";

export async function createInstanceProfile(
    name: string,
    tags: Record<string, string>
) {
    // Create IAM role for EC2 instances
    const role = new aws.iam.Role(`${name}-role`, {
        assumeRolePolicy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [{
                Action: "sts:AssumeRole",
                Effect: "Allow",
                Principal: {
                    Service: "ec2.amazonaws.com",
                },
            }],
        }),
        tags: {
            ...tags,
            Name: `${name}-role`,
        },
    });

    // Attach policies for CloudWatch, S3, Secrets Manager, and SSM
    const cloudWatchPolicy = new aws.iam.RolePolicyAttachment(`${name}-cloudwatch-policy`, {
        role: role.name,
        policyArn: "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
    });

    const ssmPolicy = new aws.iam.RolePolicyAttachment(`${name}-ssm-policy`, {
        role: role.name,
        policyArn: "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore",
    });

    const cloudWatchAgentPolicy = new aws.iam.RolePolicyAttachment(`${name}-cloudwatch-agent-policy`, {
        role: role.name,
        policyArn: "arn:aws:iam::aws:policy/CloudWatchAgentServerPolicy",
    });

    // Attach ECS policy for container instances
    const ecsPolicy = new aws.iam.RolePolicyAttachment(`${name}-ecs-policy`, {
        role: role.name,
        policyArn: "arn:aws:iam::aws:policy/service-role/AmazonEC2ContainerServiceforEC2Role",
    });

    // Custom policy for S3 and Secrets Manager access
    const customPolicy = new aws.iam.Policy(`${name}-custom-policy`, {
        description: "Custom policy for Bento cluster access",
        policy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [
                {
                    Effect: "Allow",
                    Action: [
                        "s3:GetObject",
                        "s3:PutObject",
                        "s3:DeleteObject",
                        "s3:ListBucket",
                    ],
                    Resource: [
                        `arn:aws:s3:::${name}-*/*`,
                        `arn:aws:s3:::${name}-*`,
                    ],
                },
                {
                    Effect: "Allow",
                    Action: [
                        "secretsmanager:GetSecretValue",
                        "secretsmanager:DescribeSecret",
                    ],
                    Resource: `arn:aws:secretsmanager:*:*:secret:${name}/*`,
                },
                {
                    Effect: "Allow",
                    Action: [
                        "logs:CreateLogGroup",
                        "logs:CreateLogStream",
                        "logs:PutLogEvents",
                    ],
                    Resource: "arn:aws:logs:*:*:*",
                },
                {
                    Effect: "Allow",
                    Action: [
                        "ecs:CreateCluster",
                        "ecs:DeregisterContainerInstance",
                        "ecs:DiscoverPollEndpoint",
                        "ecs:Poll",
                        "ecs:RegisterContainerInstance",
                        "ecs:StartTelemetrySession",
                        "ecs:UpdateContainerInstancesState",
                        "ecs:Submit*",
                        "ecr:GetAuthorizationToken",
                        "ecr:BatchCheckLayerAvailability",
                        "ecr:GetDownloadUrlForLayer",
                        "ecr:BatchGetImage",
                    ],
                    Resource: "*",
                },
            ],
        }),
        tags: {
            ...tags,
            Name: `${name}-custom-policy`,
        },
    });

    const customPolicyAttachment = new aws.iam.RolePolicyAttachment(`${name}-custom-policy-attachment`, {
        role: role.name,
        policyArn: customPolicy.arn,
    });

    const dbPolicy = new aws.iam.Policy(`${name}-db-policy`, {
        description: "Policy for ECS tasks to access RDS database",
        policy: pulumi.all([name]).apply(([name]) => JSON.stringify({
            Version: "2012-10-17",
            Statement: [
                {
                    Effect: "Allow",
                    Action: [
                        "rds-db:connect"
                    ],
                    Resource: `arn:aws:rds-db:*:*:dbuser:*/*`,
                },
            ],
        })),
        tags: {
            ...tags,
            Name: `${name}-db-policy`,
        },
    });

    new aws.iam.RolePolicyAttachment(`${name}-db-policy-attachment`, {
        role: role.name,
        policyArn: dbPolicy.arn,
    });

    // Create instance profile
    const instanceProfile = new aws.iam.InstanceProfile(`${name}-instance-profile`, {
        role: role.name,
        tags: {
            ...tags,
            Name: `${name}-instance-profile`,
        },
    });

    return instanceProfile;
}

export async function createEcsTaskRole(
    name: string,
    tags: Record<string, string>,
    secrets: any,
) {
    // Create IAM role for ECS tasks
    const taskRole = new aws.iam.Role(`${name}-task-role`, {
        assumeRolePolicy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [{
                Action: "sts:AssumeRole",
                Effect: "Allow",
                Principal: {
                    Service: "ecs-tasks.amazonaws.com",
                },
            }],
        }),
        tags: {
            ...tags,
            Name: `${name}-task-role`,
        },
    });

    // Create execution role for ECS
    const executionRole = new aws.iam.Role(`${name}-execution-role`, {
        assumeRolePolicy: JSON.stringify({
            Version: "2012-10-17",
            Statement: [{
                Action: "sts:AssumeRole",
                Effect: "Allow",
                Principal: {
                    Service: "ecs-tasks.amazonaws.com",
                },

            }],
        }),
        tags: {
            ...tags,
            Name: `${name}-execution-role`,
        },
    });

    const containerEc2Attachment = new aws.iam.RolePolicyAttachment(`${name}-task-container-ec2-attachment`, {
        role: taskRole.name,
        policyArn: "arn:aws:iam::aws:policy/service-role/AmazonEC2ContainerServiceforEC2Role",
    });

    const customPolicy = new aws.iam.Policy(`${name}-execution-custom-policy`, {
        description: "Custom policy for docker img pull access",
        policy: pulumi.all([secrets.dockerToken]).apply(([dockerToken]) => JSON.stringify({
            Version: "2012-10-17",
            Statement: [
                {
                    Effect: "Allow",
                    Action: [
                        "secretsmanager:GetSecretValue",
                    ],
                    Resource: dockerToken,
                },
            ],
        })),
        tags: {
            ...tags,
            Name: `${name}-custom-policy`,
        },
    });

    const customPolicyAttachment = new aws.iam.RolePolicyAttachment(`${name}-execution-custom-policy-attachment`, {
        role: executionRole.name,
        policyArn: customPolicy.arn,
    });

    // Attach basic execution policy
    const executionPolicyAttachment = new aws.iam.RolePolicyAttachment(`${name}-execution-policy`, {
        role: executionRole.name,
        policyArn: "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy",
    });

    return {
        taskRole,
        executionRole,
    };
}