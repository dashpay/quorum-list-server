output "bastion_public_ip" {
  description = "Public IP of the bastion host"
  value       = aws_instance.bastion.public_ip
}

output "dash_core_private_ip" {
  description = "Private IP of the Dash Core node"
  value       = aws_instance.dash_core.private_ip
}

output "quorum_server_private_ip" {
  description = "Private IP of the Quorum server"
  value       = aws_instance.quorum_server.private_ip
}

output "quorum_server_id" {
  description = "Instance ID of the Quorum server"
  value       = aws_instance.quorum_server.id
}

output "alb_security_group_id" {
  description = "Security group ID for the ALB"
  value       = aws_security_group.alb.id
}