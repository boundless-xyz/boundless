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
    tags: Record<string, string>
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