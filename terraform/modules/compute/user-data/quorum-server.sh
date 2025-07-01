#!/bin/bash
set -e

# Update system
apt-get update
apt-get upgrade -y

# Install dependencies
apt-get install -y curl wget git build-essential pkg-config libssl-dev

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source $HOME/.cargo/env

# Create app user
useradd -m -s /bin/bash quorum

# Clone and build the quorum-list-server
cd /home/quorum
git clone https://github.com/dashpay/quorum-list-server.git
cd quorum-list-server

# Build the application
/root/.cargo/bin/cargo build --release

# Create config directory
mkdir -p /etc/quorum-list-server

# Create configuration file
cat > /etc/quorum-list-server/config.toml << EOF
[server]
port = 8080
host = "0.0.0.0"

[rpc]
url = "http://${dash_core_ip}:19998"
username = "dashrpc"
password = "DashTestnet2024SecureRPCPassword!"

[quorum]
previous_blocks_offset = 8
EOF

# Copy binary to system location
cp target/release/quorum-list-server /usr/local/bin/
chmod +x /usr/local/bin/quorum-list-server

# Create systemd service
cat > /etc/systemd/system/quorum-list-server.service << 'EOF'
[Unit]
Description=Quorum List Server
After=network.target

[Service]
Type=simple
User=quorum
Group=quorum
WorkingDirectory=/home/quorum
Environment="RUST_LOG=info"
ExecStart=/usr/local/bin/quorum-list-server /etc/quorum-list-server/config.toml
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

# Set permissions
chown -R quorum:quorum /home/quorum/quorum-list-server
chown -R quorum:quorum /etc/quorum-list-server
chmod 600 /etc/quorum-list-server/config.toml

# Wait for Dash Core to be ready
echo "Waiting for Dash Core to be ready..."
for i in {1..60}; do
  if nc -z ${dash_core_ip} 19998; then
    echo "Dash Core is ready"
    break
  fi
  echo "Waiting for Dash Core... ($i/60)"
  sleep 10
done

# Enable and start the service
systemctl daemon-reload
systemctl enable quorum-list-server
systemctl start quorum-list-server

# Install monitoring
apt-get install -y htop iotop sysstat

echo "Quorum List Server installation completed"