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
    usePrebuilt?: boolean;
    prebuiltTag?: string;
    environmentVariables?: { [key: string]: pulumi.Input<string> };
    memorySize?: number;
    timeout?: number;
    role: pulumi.Input<string>;
    reservedConcurrentExecutions?: number;
    vpcConfig?: {
        subnetIds: pulumi.Input<pulumi.Input<string>[]>;
        securityGroupIds: pulumi.Input<pulumi.Input<string>[]>;
    };
    nameSuffix?: string;
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

export function createRustLambda(
    name: string,
    options: RustLambdaOptions,
    resourceOptions?: pulumi.CustomResourceOptions,
): { lambda: aws.lambda.Function, logGroupName: pulumi.Output<string> } {
    const nameSuffix = options.nameSuffix ?? '';
    const zipFilePath = path.join(
        options.projectPath,
        'target',
        'lambda',
        options.packageName,
        'bootstrap.zip'
    );

    if (options.usePrebuilt) {
        const tag = options.prebuiltTag ?? `nightly-${child_process
            .execSync('git rev-parse HEAD')
            .toString().trim().substring(0, 7)}`;
        const image = `ghcr.io/boundless-xyz/boundless/lambda-${options.packageName}:${tag}`;

        // Ensure target directory exists
        const zipDir = path.dirname(zipFilePath);
        if (!fs.existsSync(zipDir)) {
            fs.mkdirSync(zipDir, { recursive: true });
        }

        // Poll for image availability (CI may still be building)
        const maxAttempts = 30;
        let found = false;
        for (let i = 1; i <= maxAttempts; i++) {
            try {
                child_process.execSync(`docker manifest inspect ${image}`, { stdio: 'ignore' });
                found = true;
                break;
            } catch { /* not available yet */ }
            if (i < maxAttempts) {
                console.log(`Waiting for pre-built Lambda ${image} (attempt ${i}/${maxAttempts}, retrying in 30s)...`);
                child_process.execSync('sleep 30');
            }
        }
        if (!found) {
            throw new Error(`Pre-built Lambda image not found after ${maxAttempts} attempts: ${image}`);
        }

        // Extract zip from the image
        console.log(`Extracting pre-built Lambda ${options.packageName} from ${image}...`);
        try {
            const container = `tmp-lambda-${options.packageName}-${Date.now()}`;
            child_process.execSync(`docker create --name ${container} ${image}`, { stdio: 'ignore' });
            child_process.execSync(`docker cp ${container}:/bootstrap.zip ${zipFilePath}`, { stdio: 'inherit' });
            child_process.execSync(`docker rm ${container}`, { stdio: 'ignore' });
        } catch (error) {
            throw new Error(`Failed to extract pre-built Lambda from ${image}`);
        }
    } else {
        ensureCargoLambdaInstalled();

        const release = options.release ?? true;
        const buildMode = release ? '--release' : '';

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
    }

    if (!fs.existsSync(zipFilePath)) {
        throw new Error(`Lambda zip not found at ${zipFilePath}`);
    }

    const logGroup = new aws.cloudwatch.LogGroup(`${name}-log-group`, {
        name: `${name}`,
    });

    // Create the Lambda function with all configuration options
    const lambdaArgs: aws.lambda.FunctionArgs = {
        code: new pulumi.asset.FileArchive(zipFilePath),
        name: `${name}-lambda-${nameSuffix}`,
        loggingConfig: {
            logGroup: logGroup.name,
            logFormat: 'JSON',
            // This is the minimum log level that will be forwarded to Cloudwatch. 
            // Logging is still ultimately controlled by the RUST_LOG environment variable.
            applicationLogLevel: 'TRACE',
            systemLogLevel: 'INFO'
        },
        handler: 'bootstrap',
        runtime: 'provided.al2023',
        role: options.role,
        memorySize: options.memorySize || 128,
        timeout: options.timeout || 30,
        reservedConcurrentExecutions: options.reservedConcurrentExecutions,
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

    const lambdaResourceOptions = pulumi.mergeOptions(resourceOptions, {
        dependsOn: [logGroup],
    });

    const lambda = new aws.lambda.Function(`${name}-lambda-${nameSuffix}`, lambdaArgs, lambdaResourceOptions);
    const logGroupName = logGroup.name;

    // Create the Lambda function
    return { lambda, logGroupName };
}