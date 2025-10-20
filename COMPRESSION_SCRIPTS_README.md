# Report Compression Scripts

These bash scripts compress old daily report folders for the ant-rewards-service.

## Overview

The service generates daily reports in three categories:
- `peers_addrs/` - Peer address reports
- `peers_data/` - Peer data reports  
- `distribution_stats/` - Distribution statistics reports

Each category contains daily folders named in `YYYYMMDD` format (e.g., `20251020/`). These scripts compress old folders (prior to the current date) into `.tar.gz` archives to save disk space.

## Architecture

The scripts follow a simple, maintainable design:
- **`compress_old_reports_once.sh`** - Contains all compression logic; runs once and exits
- **`compress_old_reports.sh`** - Periodic wrapper that calls the once script at regular intervals

This design avoids code duplication and makes maintenance easier since all compression logic lives in a single place.

## Scripts

### 1. `compress_old_reports.sh` - Continuous Service

This script runs continuously, calling `compress_old_reports_once.sh` at regular intervals (default: weekly). This wrapper design avoids code duplication and makes maintenance easier.

**Usage:**

```bash
# Make both scripts executable
chmod +x compress_old_reports.sh
chmod +x compress_old_reports_once.sh

# Run with default interval (1 week)
./compress_old_reports.sh

# Run with custom interval (e.g., 1 day = 86400 seconds)
COMPRESSION_INTERVAL=86400 ./compress_old_reports.sh

# Run in background
nohup ./compress_old_reports.sh > compression.log 2>&1 &
```

**Environment Variables:**
- `COMPRESSION_INTERVAL` - Time between compression cycles in seconds (default: 604800 = 7 days)

### 2. `compress_old_reports_once.sh` - One-time Run

This script runs once and exits. Ideal for manual runs.

**Usage:**

```bash
# Make the script executable
chmod +x compress_old_reports_once.sh

# Run once
./compress_old_reports_once.sh
```

## Example

### Before Compression (run on 2025-10-20):
```
peers_addrs/20251018/
peers_addrs/20251019/
peers_addrs/20251020/
peers_data/20251018/
peers_data/20251019/
peers_data/20251020/
distribution_stats/20251018/
distribution_stats/20251019/
distribution_stats/20251020/
```

### After Compression:
```
peers_addrs/20251018.tar.gz
peers_addrs/20251019.tar.gz
peers_addrs/20251020/           ← Current day, not compressed
peers_data/20251018.tar.gz
peers_data/20251019.tar.gz
peers_data/20251020/            ← Current day, not compressed
distribution_stats/20251018.tar.gz
distribution_stats/20251019.tar.gz
distribution_stats/20251020/    ← Current day, not compressed
```

## Features

- **Safe Compression**: Verifies archive creation before removing original folders
- **Current Date Protection**: Never compresses the current day's folder
- **Error Handling**: Keeps original folders if compression fails
- **Progress Logging**: Shows detailed progress and results
- **Flexible Scheduling**: Choose between continuous service or cron-based scheduling
- **Size Reporting**: Displays compressed archive sizes

## Running as a System Service (Linux)

To run the continuous script as a systemd service:

1. Create service file: `/etc/systemd/system/report-compression.service`

```ini
[Unit]
Description=Report Compression Service
After=network.target

[Service]
Type=simple
User=your-username
WorkingDirectory=/path/to/ant-rewards-service
Environment="COMPRESSION_INTERVAL=604800"
ExecStart=/path/to/ant-rewards-service/compress_old_reports.sh
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

2. Enable and start the service:

```bash
sudo systemctl daemon-reload
sudo systemctl enable report-compression.service
sudo systemctl start report-compression.service

# Check status
sudo systemctl status report-compression.service

# View logs
sudo journalctl -u report-compression.service -f
```

## Troubleshooting

**Error: "Cannot find compress_old_reports_once.sh":**
- Both scripts must be in the same directory
- Ensure both scripts are present and have not been moved separately

**Script doesn't find folders:**
- Ensure you run the script from the ant-rewards-service root directory
- Check that the report type folders (`peers_addrs`, `peers_data`, `distribution_stats`) exist

**Permission errors:**
- Ensure both scripts have execute permissions: `chmod +x compress_old_reports*.sh`
- Ensure you have write permissions in the report directories

**Compression fails:**
- Check available disk space
- Verify `tar` is installed and accessible
- Check the error messages in the script output
