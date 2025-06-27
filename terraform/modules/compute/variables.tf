variable "environment" {
  description = "Environment name"
  type        = string
}

variable "vpc_id" {
  description = "VPC ID"
  type        = string
}

variable "public_subnet_ids" {
  description = "List of public subnet IDs"
  type        = list(string)
}

variable "private_subnet_ids" {
  description = "List of private subnet IDs"
  type        = list(string)
}

variable "key_name" {
  description = "AWS key pair name"
  type        = string
}

variable "dash_core_instance_type" {
  description = "Instance type for Dash Core node"
  type        = string
}

variable "quorum_server_instance_type" {
  description = "Instance type for Quorum server"
  type        = string
}

variable "bastion_instance_type" {
  description = "Instance type for bastion host"
  type        = string
}