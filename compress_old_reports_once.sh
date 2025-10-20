#!/bin/bash

# One-time compression script for old daily report folders
# This script compresses folders with dates prior to today and then exits

# Array of report types to process
REPORT_TYPES=("peers_addrs" "peers_data" "distribution_stats")

# Get current date in YYYYMMDD format
CURRENT_DATE=$(date +%Y%m%d)

echo "═══════════════════════════════════════════════════════════"
echo "Starting compression of old daily folders"
echo "Run time: $(date '+%Y-%m-%d %H:%M:%S')"
echo "Current date: $CURRENT_DATE"
echo "═══════════════════════════════════════════════════════════"
echo ""

# Function to compress and remove old daily folders
compress_old_folders() {
    local report_type=$1
    local compressed_count=0
    
    # Check if the report type directory exists
    if [ ! -d "$report_type" ]; then
        echo "Warning: Directory $report_type does not exist, skipping..."
        echo ""
        return
    fi
    
    echo "Processing $report_type..."
    
    # Iterate through all subdirectories in the report type folder
    for daily_folder in "$report_type"/*/ ; do
        # Check if any directories exist (handle case where glob doesn't match)
        [ -d "$daily_folder" ] || continue
        
        # Remove trailing slash and get just the folder name
        daily_folder=${daily_folder%/}
        folder_name=$(basename "$daily_folder")
        
        # Check if folder name matches YYYYMMDD format (8 digits)
        if [[ $folder_name =~ ^[0-9]{8}$ ]]; then
            # Compare folder date with current date
            if [ "$folder_name" -lt "$CURRENT_DATE" ]; then
                echo "  Compressing $daily_folder..."
                
                # Create tar.gz file in the parent directory (report_type folder)
                tar_file="$report_type/$folder_name.tar.gz"
                
                # Compress the folder (using relative path for cleaner archive)
                tar -czf "$tar_file" -C "$report_type" "$folder_name" 2>/dev/null
                
                # Check if compression was successful
                if [ $? -eq 0 ] && [ -f "$tar_file" ]; then
                    # Verify the archive is not empty
                    tar_size=$(stat -f%z "$tar_file" 2>/dev/null || stat -c%s "$tar_file" 2>/dev/null)
                    if [ "$tar_size" -gt 0 ]; then
                        echo "  ✓ Successfully created $tar_file ($(du -h "$tar_file" | cut -f1))"
                        
                        # Remove the original folder
                        rm -rf "$daily_folder"
                        
                        if [ $? -eq 0 ]; then
                            echo "  ✓ Successfully removed $daily_folder"
                            ((compressed_count++))
                        else
                            echo "  ✗ Error: Failed to remove $daily_folder"
                        fi
                    else
                        echo "  ✗ Error: Archive file is empty, keeping original folder"
                        rm -f "$tar_file"
                    fi
                else
                    echo "  ✗ Error: Failed to create $tar_file, keeping original folder"
                fi
            else
                echo "  → Skipping $daily_folder (current or future date)"
            fi
        fi
    done
    
    echo "  Summary: Compressed $compressed_count folder(s) in $report_type"
    echo ""
}

# Process each report type
for report_type in "${REPORT_TYPES[@]}"; do
    compress_old_folders "$report_type"
done

echo "═══════════════════════════════════════════════════════════"
echo "Compression completed at $(date '+%Y-%m-%d %H:%M:%S')"
echo "═══════════════════════════════════════════════════════════"

