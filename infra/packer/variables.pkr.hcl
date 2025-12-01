# Packer variables for Bento AMI builds
# This file contains variable definitions for different environments

# Common variables
aws_region = "us-west-2"
instance_type = "c7a.4xlarge"
boundless_bento_version = "v1.1.2"
boundless_broker_version = "v1.1.2"

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
