import * as pulumi from "@pulumi/pulumi";
import { ProverClusterPipeline } from "./prover-cluster";
import { BasePipelineArgs } from "./base";

interface ProverClusterStagingPipelineArgs extends BasePipelineArgs {
    stagingAccountId: string;
    opsAccountId: string;
    amiId?: string;
}

export class ProverClusterStagingPipeline extends pulumi.ComponentResource {
    constructor(name: string, args: ProverClusterStagingPipelineArgs, opts?: pulumi.ComponentResourceOptions) {
        super("pulumi:aws:prover-cluster-staging-pipeline", name, args, opts);

        const { artifactBucket, connection, stagingAccountId, opsAccountId, amiId } = args;

        // Create staging prover cluster pipeline
        const stagingPipeline = new ProverClusterPipeline("staging-prover-cluster", {
            artifactBucket,
            connection,
            role: args.role,
            environment: "staging",
            serviceAccountId: stagingAccountId,
            opsAccountId,
            amiId,
            githubToken: args.githubToken,
            dockerUsername: args.dockerUsername,
            dockerToken: args.dockerToken,
            slackAlertsTopicArn: args.slackAlertsTopicArn,
        }, { parent: this });

        // Outputs
        this.pipelineName = stagingPipeline.pipelineName;
        this.buildProjectName = stagingPipeline.buildProjectName;
    }

    public readonly pipelineName!: pulumi.Output<string>;
    public readonly buildProjectName!: pulumi.Output<string>;
}
