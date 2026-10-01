# Packs a release build into dist/rsit-VERSION-x86_64-windows.zip.
# Usage: script/package-windows.ps1 VERSION [BINARY]
param(
    [Parameter(Mandatory)] [string] $Version,
    [string] $Binary = "target/release/rsit.exe"
)
$ErrorActionPreference = "Stop"

$name = "rsit-$Version-x86_64-windows"
$dir = "dist/$name"
if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item $Binary "$dir/rsit.exe"
Copy-Item NOTICE "$dir/"

$zip = "dist/$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $dir -DestinationPath $zip
Write-Output $zip
