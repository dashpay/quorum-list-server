# Quorum List Server AWS Infrastructure

This Terraform configuration deploys the Quorum List Server infrastructure on AWS.

## Infrastructure Components

- **VPC**: Custom VPC with public and private subnets across 2 AZs
- **Dash Core Node**: t3.medium instance running in private subnet
- **Quorum Server**: t3.micro instance running the Rust application in private subnet
- **Bastion Host**: t3.micro instance in public subnet for SSH access
- **Application Load Balancer**: Public-facing ALB with SSL termination
- **Route 53**: DNS configuration for quorums.testnet.networks.dash.org
- **ACM**: SSL certificate for HTTPS

## Prerequisites

1. AWS CLI configured with appropriate credentials
2. Terraform installed (version >= 1.0)
3. SSH key pair named "dashdev" already exists in AWS
4. Access to Route 53 zone for networks.dash.org

## Deployment Steps

1. Navigate to the testnet environment:
   ```bash
   cd terraform/environments/testnet
   ```

2. Initialize Terraform:
   ```bash
   terraform init
   ```

3. Review the plan:
   ```bash
   terraform plan
   ```

4. Apply the configuration:
   ```bash
   terraform apply
   ```

5. Note the outputs, especially the bastion host IP for SSH access.

## Accessing the Infrastructure

### SSH to Bastion Host
```bash
ssh -i ~/.ssh/dashdev.pem ubuntu@<bastion-public-ip>
```

### SSH to Dash Core Node (via bastion)
```bash
ssh -i ~/.ssh/dashdev.pem -J ubuntu@<bastion-public-ip> ubuntu@<dash-core-private-ip>
```

### SSH to Quorum Server (via bastion)
```bash
ssh -i ~/.ssh/dashdev.pem -J ubuntu@<bastion-public-ip> ubuntu@<quorum-server-private-ip>
```

### Check Service Status
On the Dash Core node:
```bash
sudo systemctl status dashd
sudo -u dash dash-cli -testnet getblockchaininfo
```

On the Quorum Server:
```bash
sudo systemctl status quorum-list-server
sudo journalctl -u quorum-list-server -f
```

## API Endpoints

Once deployed, the Quorum List Server API will be available at:
- https://quorums.testnet.networks.dash.org/info
- https://quorums.testnet.networks.dash.org/quorums

## Security Notes

1. Update the RPC password in both user-data scripts before deployment
2. The bastion host allows SSH from anywhere (0.0.0.0/0) - consider restricting this
3. All internal communication happens over private IPs
4. The ALB only accepts HTTPS traffic (HTTP redirects to HTTPS)

## Cleanup

To destroy all resources:
```bash
terraform destroy
```

## Customization

Key variables can be modified in `variables.tf`:
- Instance types
- VPC CIDR blocks
- AWS region
- Domain name