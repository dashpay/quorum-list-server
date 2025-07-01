output "alb_dns_name" {
  description = "DNS name of the Application Load Balancer"
  value       = module.loadbalancer.alb_dns_name
}

output "bastion_public_ip" {
  description = "Public IP of the bastion host"
  value       = module.compute.bastion_public_ip
}

output "dash_core_private_ip" {
  description = "Private IP of the Dash Core node"
  value       = module.compute.dash_core_private_ip
}

output "quorum_server_private_ip" {
  description = "Private IP of the Quorum server"
  value       = module.compute.quorum_server_private_ip
}

output "vpc_id" {
  description = "ID of the VPC"
  value       = module.network.vpc_id
}

output "domain_name" {
  description = "Domain name pointing to the load balancer"
  value       = var.domain_name
}