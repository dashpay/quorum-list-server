output "certificate_arn" {
  description = "ARN of the SSL certificate"
  value       = aws_acm_certificate_validation.main.certificate_arn
}

output "domain_name" {
  description = "The domain name"
  value       = var.domain_name
}

output "route53_record_fqdn" {
  description = "FQDN of the Route53 record"
  value       = aws_route53_record.quorum_server.fqdn
}