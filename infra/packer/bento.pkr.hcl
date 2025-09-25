packer {
  required_plugins {
    amazon = {
      version = ">= 1.0.0"
      source  = "github.com/hashicorp/amazon"
    }
  }
}

variable "aws_region" {
  type    = string
  default = "us-west-2"
}

variable "instance_type" {
  type    = string
  default = "c7a.4xlarge"
}

variable "boundless_version" {
  type    = string
  default = "latest"
}

variable "docker_tag" {
  type    = string
  default = "latest"
}

variable "service_account_ids" {
  type = list(string)
  default = []
  description = "List of AWS account IDs to share the AMI with"
}

variable "environment" {
  type    = string
  default = "production"
  description = "Environment name for tagging"
}

locals {
  timestamp = regex_replace(timestamp(), "[- TZ:]", "")
}

source "amazon-ebs" "boundless" {
  ami_name      = "boundless-${var.boundless_version}-ubuntu-24.04-nvidia-${local.timestamp}"
  instance_type = var.instance_type
  region        = var.aws_region
  source_ami_filter {
    filters = {
      name                = "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"
      root-device-type    = "ebs"
      virtualization-type = "hvm"
    }
    most_recent = true
    owners      = ["099720109477"] # Canonical
  }
  ssh_username = "ubuntu"

  # Increase root volume size to 100GB
  launch_block_device_mappings {
    device_name = "/dev/sda1"
    volume_size = 25
    volume_type = "gp3"
    delete_on_termination = true
  }

  # Share AMI with service accounts
  ami_users = var.service_account_ids

  tags = {
    Name        = "boundless-${var.boundless_version}-ubuntu-24.04-nvidia-13.0"
    Environment = var.environment
    ManagedBy   = "packer"
    Version     = var.boundless_version
    BuildDate   = local.timestamp
  }
}

build {
  sources = ["source.amazon-ebs.boundless"]

  provisioner "file" {
    source = "./config_files/vector.yaml"
    destination = "/tmp/vector.yaml"
  }

  # Copy service files first
  provisioner "file" {
    source = "./service_files/bento-api.service"
    destination = "/tmp/bento-api.service"
  }

  provisioner "file" {
    source = "./service_files/bento-broker.service"
    destination = "/tmp/bento-broker.service"
  }

  provisioner "file" {
    source = "./service_files/broker.toml"
    destination = "/tmp/broker.toml"
  }

  provisioner "file" {
    source = "./service_files/bento-executor.service"
    destination = "/tmp/bento-executor.service"
  }

  provisioner "file" {
    source = "./service_files/bento-aux.service"
    destination = "/tmp/bento-aux.service"
  }

  provisioner "file" {
    source = "./service_files/bento-prover.service"
    destination = "/tmp/bento-prover.service"
  }

  # Run the complete installation script
  provisioner "shell" {
    script = "scripts/setup.sh"
  }
}
