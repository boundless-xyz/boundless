import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import * as path from "path";
import * as fs from "fs";
import * as child_process from "child_process";
import { BaseComponent, BaseComponentConfig } from "./BaseComponent";

export interface ScalerComponentConfig extends BaseComponentConfig {
    rdsEndpoint: pulumi.Output<string>;
    rdsSecurityGroupId: pulumi.Output<string>;
    taskDBName: string;
    taskDBUsername: string;
    taskDBPassword: string;
    proverAsgName: pulumi.Output<string>;
    proverAsgArn: pulumi.Output<string>;
    scheduleExpression?: string;
}

export class ScalerComponent extends BaseComponent {
    public readonly lambdaFunction: aws.lambda.Function;
    public readonly lambdaRole: aws.iam.Role;
    public readonly eventRule: aws.cloudwatch.EventRule;
    public readonly eventTarget: aws.cloudwatch.EventTarget;

    constructor(config: ScalerComponentConfig) {
        super(config, "boundless-bento");

        // Create IAM role for Lambda
        this.lambdaRole = this.createLambdaRole(config);

        // Create security group for Lambda to access RDS
        const lambdaSecurityGroup = this.createLambdaSecurityGroup(config);

        // Allow Lambda security group to access RDS
        // AWS Lambda RDS connection requires:
        // 1. Ingress rule on RDS security group to allow traffic from Lambda
        const rdsIngressRule = new aws.ec2.SecurityGroupRule(
            `${config.stackName}-lambda-rds-ingress`,
            {
                type: "ingress",
                fromPort: 5432,
                toPort: 5432,
                protocol: "tcp",
                securityGroupId: config.rdsSecurityGroupId,
                sourceSecurityGroupId: lambdaSecurityGroup.id,
                description: "Allow Lambda to access RDS",
            },
            {
                dependsOn: [lambdaSecurityGroup],
            }
        );

        // Note: Lambda SG already has egress for all traffic, but AWS Lambda's RDS connection
        // detection requires an explicit egress :/ so here it is.
        const lambdaEgressRule = new aws.ec2.SecurityGroupRule(
            `${config.stackName}-lambda-rds-egress`,
            {
                type: "egress",
                fromPort: 5432,
                toPort: 5432,
                protocol: "tcp",
                securityGroupId: lambdaSecurityGroup.id,
                cidrBlocks: ["0.0.0.0/0"], // Allow egress to RDS port
                description: "Allow Lambda to connect to RDS",
            },
            {
                dependsOn: [lambdaSecurityGroup],
            }
        );

        // Construct database URL
        const databaseUrl = pulumi.all([config.rdsEndpoint, config.taskDBName, config.taskDBUsername, config.taskDBPassword])
            .apply(([endpoint, dbName, username, password]) => {
                // RDS endpoint format: hostname:port
                const [host, port] = endpoint.split(":");
                return `postgresql://${username}:${password}@${host}:${port}/${dbName}`;
            });

        // Create Lambda layer with Python dependencies
        const lambdaLayer = this.createLambdaLayer(config);

        // Create CloudWatch log group
        const logGroup = new aws.cloudwatch.LogGroup(
            `${config.stackName}-scaler-log-group`,
            {
                name: `/aws/lambda/${this.generateName("scaler")}`,
                retentionInDays: 7,
                tags: {
                    Environment: config.environment,
                    Stack: config.stackName,
                    Component: "scaler",
                },
            }
        );

        // Create Lambda deployment package (zip with just the Python handler)
        const scalerDir = path.join(__dirname, "../../scaler");
        const lambdaFunctionPath = path.join(scalerDir, "lambda_function.py");

        if (!fs.existsSync(lambdaFunctionPath)) {
            throw new Error(`Lambda handler not found at ${lambdaFunctionPath}`);
        }

        // Create a zip file with just lambda_function.py
        const lambdaZipPath = path.join(scalerDir, "lambda.zip");
        try {
            // Always remove old zip to ensure we get the latest code
            if (fs.existsSync(lambdaZipPath)) {
                fs.unlinkSync(lambdaZipPath);
            }
            // Create zip with lambda_function.py at the root
            // Use -j to junk paths and put file at root of zip
            child_process.execSync(
                `cd ${scalerDir} && zip -j ${lambdaZipPath} lambda_function.py`,
                { stdio: "inherit" }
            );
            console.log(`Created Lambda zip at ${lambdaZipPath}`);
        } catch (error) {
            throw new Error(`Failed to create Lambda zip file: ${error}`);
        }

        if (!fs.existsSync(lambdaZipPath)) {
            throw new Error(`Lambda zip file was not created at ${lambdaZipPath}`);
        }

        // Use the zip file for Lambda code
        const lambdaCode = new pulumi.asset.FileArchive(lambdaZipPath);

        // Create Lambda function with Python code
        this.lambdaFunction = new aws.lambda.Function(
            `${config.stackName}-scaler-lambda`,
            {
                name: this.generateName("scaler"),
                runtime: "python3.12",
                handler: "lambda_function.lambda_handler",
                role: this.lambdaRole.arn,
                code: lambdaCode,
                layers: [lambdaLayer.arn],
                memorySize: 256,
                timeout: 60,
                environment: {
                    variables: {
                        DATABASE_URL: databaseUrl,
                        AUTO_SCALING_GROUP_NAME: config.proverAsgName,
                        // Note: AWS_REGION is automatically set by Lambda and cannot be overridden
                    },
                },
                vpcConfig: pulumi.all([config.privateSubnetIds, lambdaSecurityGroup.id]).apply(([subnets, sgId]) => ({
                    subnetIds: subnets,
                    securityGroupIds: [sgId],
                })),
                tags: {
                    Environment: config.environment,
                    Stack: config.stackName,
                    Component: "scaler",
                },
            },
            {
                dependsOn: [logGroup, lambdaLayer, lambdaSecurityGroup, rdsIngressRule, lambdaEgressRule],
            }
        );

        // Create EventBridge rule to trigger Lambda on schedule
        const schedule = config.scheduleExpression || "rate(60 seconds)";
        this.eventRule = new aws.cloudwatch.EventRule(
            `${config.stackName}-scaler-schedule`,
            {
                description: `Schedule for ${config.stackName} scaler Lambda`,
                scheduleExpression: schedule,
                tags: {
                    Environment: config.environment,
                    Stack: config.stackName,
                    Component: "scaler",
                },
            }
        );

        // Add Lambda as target for EventBridge rule
        this.eventTarget = new aws.cloudwatch.EventTarget(
            `${config.stackName}-scaler-target`,
            {
                rule: this.eventRule.name,
                arn: this.lambdaFunction.arn,
            }
        );

        // Grant EventBridge permission to invoke Lambda
        new aws.lambda.Permission(
            `${config.stackName}-scaler-eventbridge-permission`,
            {
                statementId: "AllowExecutionFromEventBridge",
                action: "lambda:InvokeFunction",
                function: this.lambdaFunction.name,
                principal: "events.amazonaws.com",
                sourceArn: this.eventRule.arn,
            }
        );
    }

    private createLambdaRole(config: ScalerComponentConfig): aws.iam.Role {
        const role = new aws.iam.Role(
            `${config.stackName}-scaler-role`,
            {
                name: this.generateName("scaler-role"),
                assumeRolePolicy: aws.iam.assumeRolePolicyForPrincipal({
                    Service: "lambda.amazonaws.com",
                }),
                tags: {
                    Environment: config.environment,
                    Stack: config.stackName,
                    Component: "scaler",
                },
            }
        );

        // Attach basic Lambda execution role (for CloudWatch Logs)
        new aws.iam.RolePolicyAttachment(
            `${config.stackName}-scaler-logs`,
            {
                role: role.name,
                policyArn: aws.iam.ManagedPolicies.AWSLambdaBasicExecutionRole,
            }
        );

        // Attach VPC access role
        new aws.iam.RolePolicyAttachment(
            `${config.stackName}-scaler-vpc`,
            {
                role: role.name,
                policyArn: aws.iam.ManagedPolicies.AWSLambdaVPCAccessExecutionRole,
            }
        );

        new aws.iam.RolePolicy(
            `${config.stackName}-scaler-autoscaling-policy`,
            {
                role: role.id,
                policy: pulumi.all([config.proverAsgArn]).apply(([asgArn]) =>
                    JSON.stringify({
                        Version: "2012-10-17",
                        Statement: [
                            {
                                Effect: "Allow",
                                Action: [
                                    "autoscaling:DescribeAutoScalingGroups",
                                ],
                                Resource: "*",
                            },
                            {
                                Effect: "Allow",
                                Action: [
                                    "autoscaling:UpdateAutoScalingGroup",
                                ],
                                Resource: asgArn,
                            },
                        ],
                    })
                ),
            }
        );

        return role;
    }

    private createLambdaLayer(config: ScalerComponentConfig): aws.lambda.LayerVersion {
        const scalerDir = path.join(__dirname, "../../scaler");
        const layerDir = path.join(scalerDir, "layer");
        const pythonDir = path.join(layerDir, "python");
        const requirementsPath = path.join(scalerDir, "requirements.txt");
        const layerZipPath = path.join(scalerDir, "layer.zip");

        // Use Docker to build the layer in an environment matching Lambda's runtime
        // This ensures psycopg2-binary is compiled for Amazon Linux, not the local OS
        if (fs.existsSync(requirementsPath)) {
            try {
                console.log("Building Lambda layer using Docker (Amazon Linux environment)...");

                // Clean up old layer directory and zip
                if (fs.existsSync(layerDir)) {
                    fs.rmSync(layerDir, { recursive: true, force: true });
                }
                if (fs.existsSync(layerZipPath)) {
                    fs.unlinkSync(layerZipPath);
                }

                // Create layer directory structure
                fs.mkdirSync(pythonDir, { recursive: true });

                // Use Docker with Lambda Python runtime image to build the layer
                // This ensures dependencies are compiled for Amazon Linux
                // Override the entrypoint since Lambda images expect a handler as the first argument
                const dockerCmd = `docker run --rm --entrypoint /bin/bash ` +
                    `-v "${scalerDir}:/var/task" -v "${layerDir}:/opt/layer" ` +
                    `public.ecr.aws/lambda/python:3.12 ` +
                    `-c "pip install -r /var/task/requirements.txt -t /opt/layer/python --platform manylinux2014_x86_64 --only-binary=:all: --no-cache-dir || pip install -r /var/task/requirements.txt -t /opt/layer/python --no-cache-dir"`;

                child_process.execSync(dockerCmd, { stdio: "inherit" });

                // Create zip file for layer
                child_process.execSync(
                    `cd ${layerDir} && zip -r ${layerZipPath} python/`,
                    { stdio: "inherit" }
                );
            } catch (error) {
                console.warn("Docker build failed, trying local pip install (may not work for psycopg2):", error);

                // Fallback: try local install (may fail for psycopg2 on non-Linux)
                try {
                    if (!fs.existsSync(pythonDir)) {
                        fs.mkdirSync(pythonDir, { recursive: true });
                    }
                    child_process.execSync(
                        `pip install -r ${requirementsPath} -t ${pythonDir} --platform manylinux2014_x86_64 --only-binary=:all: || pip install -r ${requirementsPath} -t ${pythonDir}`,
                        { stdio: "inherit" }
                    );
                    child_process.execSync(
                        `cd ${layerDir} && zip -r ${layerZipPath} python/`,
                        { stdio: "inherit" }
                    );
                } catch (fallbackError) {
                    throw new Error(`Failed to build Lambda layer: ${error}. Fallback also failed: ${fallbackError}`);
                }
            }
        }

        if (!fs.existsSync(layerZipPath)) {
            throw new Error(`Lambda layer zip file was not created at ${layerZipPath}`);
        }

        return new aws.lambda.LayerVersion(
            `${config.stackName}-scaler-layer`,
            {
                layerName: this.generateName("scaler-layer"),
                code: new pulumi.asset.FileArchive(layerZipPath),
                compatibleRuntimes: ["python3.11", "python3.12"],
                description: "Python dependencies for scaler Lambda",
            }
        );
    }

    private createLambdaSecurityGroup(config: ScalerComponentConfig): aws.ec2.SecurityGroup {
        return new aws.ec2.SecurityGroup(
            `${config.stackName}-scaler-sg`,
            {
                name: this.generateName("scaler-sg"),
                vpcId: config.vpcId,
                description: "Security group for scaler Lambda to access RDS",
                egress: [
                    {
                        protocol: "-1",
                        fromPort: 0,
                        toPort: 0,
                        cidrBlocks: ["0.0.0.0/0"],
                        description: "All outbound traffic",
                    },
                ],
                tags: {
                    Environment: config.environment,
                    Stack: config.stackName,
                    Component: "scaler",
                },
            }
        );
    }
}

