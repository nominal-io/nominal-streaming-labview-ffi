# Copy shared library to NI Linux RT Target
$targetIP = "192.168.86.39"
$targetUser = "admin"
$localFile = "lib_nominal-streaming-lv_64.so"
$remotePath = "/usr/local/lib/lib_nominal-streaming-lv_64.so"

# Copy the file using scp
scp $localFile "${targetUser}@${targetIP}:${remotePath}"

# Run ldconfig on the target so the library is found at runtime
ssh "${targetUser}@${targetIP}" "ldconfig"

Write-Host "Done. Library copied and ldconfig updated on $targetIP."