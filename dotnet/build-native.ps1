# Builds the Atelier engine as atelier.dll and puts it where the .NET project
# looks for it: dotnet\native\win-<arch>\.
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
cargo build --release -p atelier-ffi --manifest-path "$here\..\Cargo.toml"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$dir = Join-Path $here "native\win-$arch"
New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item "$here\..\target\release\atelier.dll" $dir
Write-Output "native\win-$arch\atelier.dll"
