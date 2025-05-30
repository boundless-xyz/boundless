import * as pulumi from '@pulumi/pulumi';
import * as aws from '@pulumi/aws';
import * as child_process from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import * as crypto from 'crypto';

export interface RustLambdaOptions {
    projectPath: string;
    packageName: string;
    release?: boolean;
    environmentVariables?: { [key: string]: pulumi.Input<string> };
    memorySize?: number;
    timeout?: number;
    role: pulumi.Input<string>;
    vpcConfig?: {
        subnetIds: pulumi.Input<pulumi.Input<string>[]>;
        securityGroupIds: pulumi.Input<pulumi.Input<string>[]>;
    };
}

/**
 * Ensures that cargo-lambda is installed, and installs it if it's not.
 */
function ensureCargoLambdaInstalled(): void {
    try {
        // Check if cargo-lambda is already installed
        const result = child_process.spawnSync('cargo', ['lambda', '--version'], {
            stdio: ['ignore', 'pipe', 'pipe'],
            encoding: 'utf-8'
        });

        if (result.status === 0) {
            console.log(`cargo-lambda is already installed: ${result.stdout.trim()}`);
            return;
        }
    } catch (error) {
        // If the command fails, we need to install cargo-lambda
        console.log('cargo-lambda not found, installing...');
    }

    // Install cargo-lambda using the official installer
    try {
        console.log('Installing cargo-lambda with the official installer');
        child_process.execSync('curl -sSf https://lambda.tools/install.sh | sh', {
            stdio: 'inherit',
        });
        console.log('cargo-lambda installed successfully');
    } catch (error) {
        console.error('Failed to install cargo-lambda:', error);
        throw new Error('Failed to install cargo-lambda. Please install it manually: https://www.cargo-lambda.info/guide/installation.html');
    }
}

export function createRustLambda(name: string, options: RustLambdaOptions): { lambda: aws.lambda.Function, logGroupName: pulumi.Output<string> } {
    ensureCargoLambdaInstalled();

    const release = options.release ?? true;
    const buildMode = release ? '--release' : '';

    // Build the package
    try {
        console.log(`Building Rust Lambda ${options.packageName} in ${options.projectPath}...`);
        child_process.execSync(
            `cd ${options.projectPath} && cargo lambda build --package ${options.packageName} ${buildMode} --output-format zip`,
            { stdio: 'inherit' }
        );
        console.log('Build successful!');
    } catch (error) {
        console.error('Build failed:', error);
        throw error;
    }

    const zipFilePath = path.join(
        options.projectPath,
        'target',
        'lambda',
        options.packageName,
        'bootstrap.zip'
    );

    if (!fs.existsSync(zipFilePath)) {
        throw new Error(`Build failed: zip file not found at ${zipFilePath}`);
    }

    const logGroup = new aws.cloudwatch.LogGroup(`${name}-log-group`, {
        name: `/aws/lambda/${name}`,
    });

    // Create the Lambda function with all configuration options
    const lambdaArgs: aws.lambda.FunctionArgs = {
        code: new pulumi.asset.FileArchive(zipFilePath),
        loggingConfig: {
            logGroup: logGroup.name,
            logFormat: 'json',
        },
        handler: 'bootstrap',
        runtime: 'provided.al2023',
        role: options.role,
        memorySize: options.memorySize || 128,
        timeout: options.timeout || 30,
        environment: options.environmentVariables ? {
            variables: options.environmentVariables,
        } : undefined,
    };

    // Add VPC configuration if provided
    if (options.vpcConfig) {
        lambdaArgs.vpcConfig = {
            subnetIds: options.vpcConfig.subnetIds,
            securityGroupIds: options.vpcConfig.securityGroupIds,
        };
    }


    const lambda = new aws.lambda.Function(`${name}-lambda`, lambdaArgs, {
        dependsOn: [logGroup],
    });
    const logGroupName = logGroup.name;

    // Create the Lambda function
    return { lambda, logGroupName };
}