#!/usr/bin/env python3
"""Prepare an explicitly retired service stack for the normal pipeline update."""

import json
import re
import subprocess
import sys


RETIRABLE_SERVICES = {"distributor", "slasher", "order-stream", "order-generator"}


def pulumi_json(*args):
    return json.loads(subprocess.check_output(["pulumi", *args], text=True))


def prepare(service, stack):
    # Shared infrastructure and indexers must never be unprotected by this path.
    if service not in RETIRABLE_SERVICES:
        return False

    config = pulumi_json("config", "--stack", stack, "--json")
    retired = config.get(f"{service}:RETIRED", {}).get("value")
    if retired not in (True, "true"):
        return False
    if not re.fullmatch(r"l-(prod|staging)-(1|8453|84532|11155111|167000)", stack):
        raise ValueError("Retirement is restricted to the named launch stacks")

    if service == "order-stream":
        indexer = pulumi_json(
            "stack", "output", "--stack", f"organization/indexer/{stack}", "--json"
        )
        if indexer.get("ORDER_STREAM_ENABLED") is not False:
            raise ValueError("Deploy the onchain-only indexer before retiring its order stream")

    checkpoint = pulumi_json("stack", "export", "--stack", stack)
    for resource in checkpoint["deployment"]["resources"]:
        if resource["urn"].split("::")[1] != service:
            raise ValueError("The selected checkpoint belongs to another project")

    # These protections previously prevented accidental deletion of the service's
    # own database and load balancer. RETIRED makes the next update remove them.
    subprocess.run(
        ["pulumi", "state", "unprotect", "--all", "--yes", "--stack", stack],
        check=True,
    )
    return True


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("Usage: prepare-retirement.py SERVICE STACK")
    prepare(*sys.argv[1:])
