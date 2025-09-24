import * as pulumi from "@pulumi/pulumi";
import { ProverClusterPipeline } from "./prover-cluster";
import { BasePipelineArgs } from "./base";

interface ProverClusterProductionPipelineArgs extends BasePipelineArgs {
    productionAccountId: string;
    opsAccountId: string;
    amiId?: string;
}

export class ProverClusterProductionPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: ProverClusterProductionPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:prover-cluster-production-pipeline", name, args, opts);

        const { artifactBucket, connection, productionAccountId, opsAccountId, amiId } = args;

        // Create production prover cluster pipeline
        const productionPipeline = new ProverClusterPipeline("production-prover-cluster", {
            artifactBucket,
            connection,
            role: args.role,
            environment: "production",
            serviceAccountId: productionAccountId,
            opsAccountId,
            amiId,
            githubToken: args.githubToken,
            dockerUsername: args.dockerUsername,
            dockerToken: args.dockerToken,
            slackAlertsTopicArn: args.slackAlertsTopicArn,
        }, { parent: this });

        // Outputs
        this.pipelineName = productionPipeline.pipelineName;
        this.buildProjectName = productionPipeline.buildProjectName;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly buildProjectName!: pulumi.Output<string>;
}
