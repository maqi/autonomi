#!/bin/bash

# Periodic compression service - calls compress_old_reports_once.sh at regular intervals
# This avoids code duplication and makes maintenance easier

# Compression interval in seconds (default: 1 week = 604800 seconds)
COMPRESSION_INTERVAL=${COMPRESSION_INTERVAL:-604800}

# Get the directory where this script is located
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ONCE_SCRIPT="$SCRIPT_DIR/compress_old_reports_once.sh"

# Check if the one-time script exists
if [ ! -f "$ONCE_SCRIPT" ]; then
    echo "Error: Cannot find compress_old_reports_once.sh in $SCRIPT_DIR"
    echo "Please ensure both scripts are in the same directory."
    exit 1
fi

# Make sure the one-time script is executable
chmod +x "$ONCE_SCRIPT"

# Main execution
echo "Report Compression Service Started"
echo "Compression interval: $COMPRESSION_INTERVAL seconds ($(($COMPRESSION_INTERVAL / 86400)) days)"
echo "Using script: $ONCE_SCRIPT"
echo ""

# Run continuously
while true; do
    # Execute the one-time compression script
    "$ONCE_SCRIPT"
    
    echo "Sleeping for $COMPRESSION_INTERVAL seconds ($(($COMPRESSION_INTERVAL / 86400)) days)..."
    echo "Next run scheduled at: $(date -d "+$COMPRESSION_INTERVAL seconds" '+%Y-%m-%d %H:%M:%S' 2>/dev/null || date -v +${COMPRESSION_INTERVAL}S '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo "N/A")"
    echo ""
    
    sleep "$COMPRESSION_INTERVAL"
done

