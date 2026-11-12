#!/usr/bin/env python3
"""Prepare an explicitly retired service stack for the normal pipeline update."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile


RETIRABLE_SERVICES = {"distributor", "slasher", "order-stream", "order-generator", "requestor-lists", "cw-monitoring"}
INACTIVE_INDEXER_STACKS = {"l-prod-84532", "l-prod-11155111"}


def prepare_checkpoint(service, stack, checkpoint):
    if service == "indexer" and stack not in INACTIVE_INDEXER_STACKS:
        raise ValueError("Active indexers must remain protected")
    deployment = checkpoint["deployment"]
    if deployment.get("pending_operations"):
        raise ValueError("Reconcile pending operations before preparing retirement")
    for resource in deployment.get("resources", []):
        urn = resource["urn"].split("::")
        if urn[0] != f"urn:pulumi:{stack}" or urn[1] != service:
            raise ValueError("The selected checkpoint belongs to another project or stack")
    for resource in deployment.get("resources", []):
        resource["protect"] = False
        # Delete options are read from saved state when the program is empty.
        for values in (resource.get("inputs", {}), resource.get("outputs", {})):
            if resource["type"] == "aws:cloudwatch/logGroup:LogGroup":
                values["skipDestroy"] = False
            elif resource["type"] == "aws:redshift/cluster:Cluster":
                values["skipFinalSnapshot"] = True
                values.pop("finalSnapshotIdentifier", None)
            elif (service == "requestor-lists" and resource["type"] == "aws:s3/bucket:Bucket") or (
                service == "indexer" and resource["type"] == "aws:s3/bucketV2:BucketV2"
            ):
                values["forceDestroy"] = True
    return checkpoint


def pulumi_json(*args):
    return json.loads(subprocess.check_output(["pulumi", *args], text=True))


def prepare(service, stack, remove_empty=False):
    # Keep shared infrastructure and every active indexer outside this path.
    if service not in RETIRABLE_SERVICES and not (
        service == "indexer" and stack in INACTIVE_INDEXER_STACKS
    ):
        return False

    config = pulumi_json("config", "--stack", stack, "--json")
    retired = config.get(f"{service}:RETIRED", {}).get("value")
    if retired not in (True, "true"):
        return False
    launch = re.fullmatch(r"l-(prod|staging)-(1|8453|84532|11155111|167000)", stack)
    legacy = service in {"distributor", "order-stream"} and re.fullmatch(r"(prod|staging)-(8453|84532|11155111)", stack)
    named = (service == "requestor-lists" and stack in {"l-prod", "l-staging"}) or (service == "cw-monitoring" and stack in {"production", "staging"})
    if not (launch or legacy or named or (service == "order-stream" and stack == "test")):
        raise ValueError("Retirement is restricted to the named service stacks")
    if not (Path(__file__).parent / service / f"Pulumi.{stack}.yaml").is_file():
        raise ValueError("No checked-in configuration for the selected stack")

    checkpoint = pulumi_json("stack", "export", "--stack", stack)
    prepare_checkpoint(service, stack, checkpoint)
    if remove_empty:
        if any(not r["type"].startswith("pulumi:") for r in checkpoint["deployment"].get("resources", [])):
            raise ValueError("Refusing to remove a stack that still owns resources")
        subprocess.run(["pulumi", "stack", "rm", "--stack", stack, "--yes", "--non-interactive", "--preserve-config"], check=True)
        return True

    if service == "order-stream":
        if stack == "test":
            if any(r.get("custom") and not r["type"].startswith("pulumi:") for r in checkpoint["deployment"].get("resources", [])):
                raise ValueError("Inspect legacy test resources before retirement")
            return False
        indexer_stack = stack if launch else f"l-{stack}"
        indexer = pulumi_json(
            "stack", "output", "--stack", f"organization/indexer/{indexer_stack}", "--json"
        )
        if indexer.get("ORDER_STREAM_ENABLED") is not False:
            raise ValueError("Deploy the onchain-only indexer before retiring its order stream")

    # Exported secrets remain encrypted. Remove the private temporary file on exit.
    with tempfile.NamedTemporaryFile(mode="w+", suffix=".json") as state:
        json.dump(checkpoint, state)
        state.flush()
        subprocess.run(["pulumi", "stack", "import", "--stack", stack,
                        "--file", state.name, "--non-interactive"], check=True)
    return True


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("service", choices=sorted(RETIRABLE_SERVICES | {"indexer"}))
    parser.add_argument("stack")
    parser.add_argument("--remove-empty", action="store_true", help="Remove the checkpoint only after all service resources are gone")
    args = parser.parse_args()
    prepare(args.service, args.stack, args.remove_empty)
