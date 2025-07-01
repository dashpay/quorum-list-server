terraform {
  required_version = ">= 1.0"
  
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
  
  backend "s3" {
    bucket = "dash-terraform-state-854439639386"
    key    = "quorum-list-server/testnet/terraform.tfstate"
    region = "us-east-1"
  }
}

provider "aws" {
  region = var.aws_region
}

module "network" {
  source = "../../modules/network"
  
  environment     = var.environment
  vpc_cidr        = var.vpc_cidr
  azs             = var.availability_zones
  public_subnets  = var.public_subnets
  private_subnets = var.private_subnets
}

module "compute" {
  source = "../../modules/compute"
  
  environment           = var.environment
  vpc_id               = module.network.vpc_id
  public_subnet_ids    = module.network.public_subnet_ids
  private_subnet_ids   = module.network.private_subnet_ids
  key_name             = var.key_name
  dash_core_instance_type    = var.dash_core_instance_type
  quorum_server_instance_type = var.quorum_server_instance_type
  bastion_instance_type      = var.bastion_instance_type
}

module "loadbalancer" {
  source = "../../modules/loadbalancer"
  
  environment           = var.environment
  vpc_id               = module.network.vpc_id
  public_subnet_ids    = module.network.public_subnet_ids
  quorum_server_id     = module.compute.quorum_server_id
  certificate_arn      = module.dns.certificate_arn
  alb_security_group_id = module.network.alb_security_group_id
}

module "dns" {
  source = "../../modules/dns"
  
  domain_name     = var.domain_name
  alb_dns_name    = module.loadbalancer.alb_dns_name
  alb_zone_id     = module.loadbalancer.alb_zone_id
}