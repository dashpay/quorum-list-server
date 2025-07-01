data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical

  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*"]
  }

  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }
}

# Security Groups
resource "aws_security_group" "bastion" {
  name        = "${var.environment}-bastion-sg"
  description = "Security group for bastion host"
  vpc_id      = var.vpc_id

  ingress {
    from_port   = 22
    to_port     = 22
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name        = "${var.environment}-bastion-sg"
    Environment = var.environment
  }
}

resource "aws_security_group" "dash_core" {
  name        = "${var.environment}-dash-core-sg"
  description = "Security group for Dash Core node"
  vpc_id      = var.vpc_id

  # SSH from bastion
  ingress {
    from_port       = 22
    to_port         = 22
    protocol        = "tcp"
    security_groups = [aws_security_group.bastion.id]
  }

  # Dash Core P2P port
  ingress {
    from_port   = 19999
    to_port     = 19999
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  # Dash Core RPC port (from Quorum server only)
  ingress {
    from_port       = 19998
    to_port         = 19998
    protocol        = "tcp"
    security_groups = [aws_security_group.quorum_server.id]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name        = "${var.environment}-dash-core-sg"
    Environment = var.environment
  }
}

resource "aws_security_group" "quorum_server" {
  name        = "${var.environment}-quorum-server-sg"
  description = "Security group for Quorum server"
  vpc_id      = var.vpc_id

  # SSH from bastion
  ingress {
    from_port       = 22
    to_port         = 22
    protocol        = "tcp"
    security_groups = [aws_security_group.bastion.id]
  }

  # HTTP from ALB
  ingress {
    from_port       = 8080
    to_port         = 8080
    protocol        = "tcp"
    security_groups = [aws_security_group.alb.id]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name        = "${var.environment}-quorum-server-sg"
    Environment = var.environment
  }
}

resource "aws_security_group" "alb" {
  name        = "${var.environment}-alb-sg"
  description = "Security group for Application Load Balancer"
  vpc_id      = var.vpc_id

  ingress {
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  ingress {
    from_port   = 80
    to_port     = 80
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = {
    Name        = "${var.environment}-alb-sg"
    Environment = var.environment
  }
}

# EC2 Instances
resource "aws_instance" "bastion" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.bastion_instance_type
  key_name              = var.key_name
  subnet_id             = var.public_subnet_ids[0]
  vpc_security_group_ids = [aws_security_group.bastion.id]

  tags = {
    Name        = "${var.environment}-bastion"
    Environment = var.environment
  }
}

resource "aws_instance" "dash_core" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.dash_core_instance_type
  key_name              = var.key_name
  subnet_id             = var.private_subnet_ids[0]
  vpc_security_group_ids = [aws_security_group.dash_core.id]

  root_block_device {
    volume_type = "gp3"
    volume_size = 100
    encrypted   = true
  }

  user_data = file("${path.module}/user-data/dash-core.sh")

  tags = {
    Name        = "${var.environment}-dash-core"
    Environment = var.environment
  }
}

resource "aws_instance" "quorum_server" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.quorum_server_instance_type
  key_name              = var.key_name
  subnet_id             = var.private_subnet_ids[0]
  vpc_security_group_ids = [aws_security_group.quorum_server.id]

  user_data = templatefile("${path.module}/user-data/quorum-server.sh", {
    dash_core_ip = aws_instance.dash_core.private_ip
  })

  depends_on = [aws_instance.dash_core]

  tags = {
    Name        = "${var.environment}-quorum-server"
    Environment = var.environment
  }
}