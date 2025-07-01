variable "environment" {
  description = "Environment name"
  type        = string
  default     = "testnet"
}

variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-1"
}

variable "vpc_cidr" {
  description = "CIDR block for VPC"
  type        = string
  default     = "10.0.0.0/16"
}

variable "availability_zones" {
  description = "Availability zones"
  type        = list(string)
  default     = ["us-east-1a", "us-east-1b"]
}

variable "public_subnets" {
  description = "Public subnet CIDR blocks"
  type        = list(string)
  default     = ["10.0.1.0/24", "10.0.2.0/24"]
}

variable "private_subnets" {
  description = "Private subnet CIDR blocks"
  type        = list(string)
  default     = ["10.0.10.0/24", "10.0.11.0/24"]
}

variable "key_name" {
  description = "AWS key pair name"
  type        = string
  default     = "dashdev.rsa"
}

variable "dash_core_instance_type" {
  description = "Instance type for Dash Core node"
  type        = string
  default     = "t3.medium"
}

variable "quorum_server_instance_type" {
  description = "Instance type for Quorum server"
  type        = string
  default     = "t3.micro"
}

variable "bastion_instance_type" {
  description = "Instance type for bastion host"
  type        = string
  default     = "t3.micro"
}

variable "domain_name" {
  description = "Domain name for the quorum server"
  type        = string
  default     = "quorums.testnet.networks.dash.org"
}