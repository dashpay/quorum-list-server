#!/bin/bash
set -e

# Update system
apt-get update
apt-get upgrade -y

# Install dependencies
apt-get install -y curl wget software-properties-common

# Create dash user
useradd -m -s /bin/bash dash

# Download and install Dash Core
DASH_VERSION="22.1.2"
cd /tmp
wget https://github.com/dashpay/dash/releases/download/v${DASH_VERSION}/dashcore-${DASH_VERSION}-x86_64-linux-gnu.tar.gz
tar -xzf dashcore-${DASH_VERSION}-x86_64-linux-gnu.tar.gz
cp dashcore-${DASH_VERSION}/bin/* /usr/local/bin/
rm -rf dashcore-${DASH_VERSION}*

# Create data directory
mkdir -p /home/dash/.dashcore
chown -R dash:dash /home/dash/.dashcore

# Create dash.conf for testnet
cat > /home/dash/.dashcore/dash.conf << 'EOF'
testnet=1
server=1
listen=1
daemon=1

# Performance settings
dbcache=1024
maxorphantx=10
maxmempool=50
maxconnections=125
maxuploadtarget=5000

# Testnet specific
[test]
rpcport=19998
port=19999
rpcuser=dashrpc
rpcpassword=DashTestnet2024SecureRPCPassword!
rpcallowip=10.0.0.0/16
rpcbind=0.0.0.0
EOF

chown dash:dash /home/dash/.dashcore/dash.conf
chmod 600 /home/dash/.dashcore/dash.conf

# Create systemd service
cat > /etc/systemd/system/dashd.service << 'EOF'
[Unit]
Description=Dash Core Daemon
After=network.target

[Service]
Type=forking
User=dash
Group=dash
WorkingDirectory=/home/dash
ExecStart=/usr/local/bin/dashd -conf=/home/dash/.dashcore/dash.conf -datadir=/home/dash/.dashcore
ExecStop=/usr/local/bin/dash-cli -conf=/home/dash/.dashcore/dash.conf -datadir=/home/dash/.dashcore stop
Restart=on-failure
RestartSec=30

[Install]
WantedBy=multi-user.target
EOF

# Enable and start Dash Core
systemctl daemon-reload
systemctl enable dashd
systemctl start dashd

# Install monitoring
apt-get install -y htop iotop sysstat

echo "Dash Core installation completed"