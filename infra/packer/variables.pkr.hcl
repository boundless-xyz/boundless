# Packer variables for Bento AMI builds
# This file contains variable definitions for different environments

# Common variables
aws_region = "us-west-2"
instance_type = "c7a.4xlarge"
boundless_version = "v1.0.1"
docker_tag = "latest"

# Environment-specific variables
environment = "production"

# Service account IDs for AMI sharing
# These will be populated by the pipeline
service_account_ids = [
  # Dev account ID
  "751442549745",
  # Staging account ID
  "245178712747",
  # Production account ID
  "632745187633"
]

# GPU-enabled instance types for prover nodes
gpu_instance_types = [
  "g6.xlarge",
  "g6.2xlarge",
  "g6.4xlarge",
  "g6.8xlarge",
  "g6.12xlarge",
  "g6.16xlarge",
  "g6.24xlarge",
  "g6.48xlarge",
  "g6e.xlarge",
  "g6e.2xlarge",
  "g6e.4xlarge",
  "g6e.8xlarge",
  "g6e.12xlarge",
  "g6e.16xlarge",
  "g6e.24xlarge",
  "g6e.48xlarge"
]

# CPU-only instance types for manager nodes
cpu_instance_types = [
  "t3.large",
  "t3.xlarge",
  "t3.2xlarge",
  "c7a.large",
  "c7a.xlarge",
  "c7a.2xlarge",
  "c7a.4xlarge",
  "c7a.8xlarge"
]
