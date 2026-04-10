$ErrorActionPreference = "Stop"

Write-Host "Building project in release mode..."
cargo build --release

$stagingDir = "viia-windows-release"
if (Test-Path $stagingDir) {
    Remove-Item -Recurse -Force $stagingDir
}
New-Item -ItemType Directory -Path $stagingDir | Out-Null

Write-Host "Copying files to staging directory..."
Copy-Item "target\release\viia.exe" -Destination $stagingDir
Copy-Item "target\release\viiaw.exe" -Destination $stagingDir
Copy-Item "target\release\WebView2Loader.dll" -Destination $stagingDir

if (Test-Path "README.md") { Copy-Item "README.md" -Destination $stagingDir }
if (Test-Path "LICENSE") { Copy-Item "LICENSE" -Destination $stagingDir }

$zipFile = "viia-windows-release.zip"
if (Test-Path $zipFile) {
    Remove-Item -Force $zipFile
}

Write-Host "Creating zip archive $zipFile..."
Compress-Archive -Path "$stagingDir\*" -DestinationPath $zipFile

Write-Host "Cleaning up staging directory..."
Remove-Item -Recurse -Force $stagingDir

Write-Host "Release packaged successfully into $zipFile"
