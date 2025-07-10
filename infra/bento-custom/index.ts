import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws";
import { setupNetwork } from "./components/network";
import { setupDatabase } from "./components/database";
import { setupStorage } from "./components/storage";
import { setupCache } from "./components/cache";
import { setupEcsCluster } from "./components/ecs-cluster";
import { setupExecAgents } from "./components/exec-agents";
import { setupSnarkAgent } from "./components/snark-agent";
import { setupGpuProvers } from "./components/gpu-provers";
import { setupAuxAgent } from "./components/aux-agent";
import { setupEC2Broker } from "./components/ec2-broker";
import { setupBentoAPI } from "./components/bento-api";
import { setupSecrets } from "./components/secrets";
import { setupMonitoring } from "./components/monitoring";

const config = new pulumi.Config();
const environment = config.get("environment") || "custom";
const region = aws.getRegion().then(r => r.name);
const rdsPassword = config.requireSecret("rdsPassword");

// Use stack name as project name for consistent naming
const projectName = pulumi.getStack();

// Create tags for all resources
const commonTags = {
    Environment: environment,
    Project: projectName,
    ManagedBy: "pulumi",
};

// Main infrastructure setup
async function main() {
    const network = await setupNetwork(projectName, commonTags);

    const secrets = await setupSecrets(projectName, commonTags);

    const database = await setupDatabase(projectName, network, commonTags, rdsPassword);

    const storage = await setupStorage(projectName, commonTags);

    // Main ECS cluster for exec/gpu/snark workers
    const cluster = await setupEcsCluster(projectName, network, commonTags);

    // Setup cache with GPU-compatible subnets for co-location
    const cache = await setupCache(projectName, network, commonTags, cluster.gpuCompatibleSubnets);

    const execAgents = await setupExecAgents(projectName, network, cluster, database, cache, storage, secrets, commonTags);
    const snarkAgent = await setupSnarkAgent(projectName, network, cluster, database, cache, storage, secrets, commonTags);
    const gpuProvers = await setupGpuProvers(projectName, network, cluster, database, cache, storage, secrets, commonTags);

    const auxAgent = await setupAuxAgent(projectName, network, cluster, database, cache, storage, secrets, commonTags);

    const bentoAPI = await setupBentoAPI(projectName, network, cluster, database, cache, storage, secrets, commonTags);

    const ec2Broker = await setupEC2Broker(projectName, network, storage, secrets, commonTags, bentoAPI.bentoApiUrl);

    // Setup monitoring and alerts
    const monitoring = await setupMonitoring(
        projectName,
        { execAgents, snarkAgent, gpuProvers, auxAgent, ec2Broker, bentoAPI },
        database,
        cache,
        cluster,
        commonTags
    );

    // Export important values
    return {
        vpcId: network.vpc.vpcId,
        // ECS cluster
        ecsClusterName: cluster.cluster.name,
        ecsClusterArn: cluster.cluster.arn,
        // ECS services
        execAgentsServiceArn: execAgents.service.id,
        snarkAgentServiceArn: snarkAgent.service.id,
        gpuProversServiceArn: gpuProvers.service.id,
        brokerInstanceArn: ec2Broker.instance.arn,
        bentoAPIServiceArn: bentoAPI.service.id,
        bentoAPIUrl: bentoAPI.bentoApiUrl,
        auxAgentServiceArn: auxAgent.service.id,
        // Database endpoints (private)
        databaseEndpoint: database.instance.endpoint,
        databaseProxyEndpoint: database.proxy.endpoint,
        redisEndpoint: cache.cluster.cacheNodes[0].address,
        s3BucketName: storage.bucket.id,
        // Monitoring
        alertsTopicArn: monitoring.alertsTopicArn,
        dashboardUrl: pulumi.interpolate`https://console.aws.amazon.com/cloudwatch/home?region=${region}#dashboards:name=${monitoring.dashboard.dashboardName}`,
    };
}

// Export the main outputs
export const outputs = main();