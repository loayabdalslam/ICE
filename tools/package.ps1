[CmdletBinding()]
param([string]$ReleaseDir)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
if (-not $ReleaseDir) { $ReleaseDir = Join-Path $project '..\ice-release' }
$release = [IO.Path]::GetFullPath($ReleaseDir)
if ($release -eq $project) { throw 'The distribution must be separate from the source project.' }
$build = Join-Path $release 'build'
New-Item -ItemType Directory -Force -Path $build | Out-Null
# Release payload hashes must survive clones with core.autocrlf enabled.
[IO.File]::WriteAllText((Join-Path $build '.gitattributes'), "* -text`n", [Text.UTF8Encoding]::new($false))
Copy-Item -LiteralPath (Join-Path $project 'target\release\ice.exe') -Destination (Join-Path $build 'ice.exe') -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'install.ps1') -Destination (Join-Path $release 'install.ps1') -Force
$brand = Join-Path $build 'brand'
New-Item -ItemType Directory -Force -Path $brand | Out-Null
foreach ($name in @('BRAND.md','logo.svg','logo-mono.svg','floe.svg','floe.png','ice.ico','identity-board.png','preview.html','welcome.png','connect.png','session.png','ice-preview.gif')) {
    Copy-Item -LiteralPath (Join-Path $project "brand\$name") -Destination (Join-Path $brand $name) -Force
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'release-readme.md') -Destination (Join-Path $build 'README.md') -Force
# Include third-party license texts with the binary distribution.
$licenses = Join-Path $build 'licenses'
New-Item -ItemType Directory -Force -Path $licenses | Out-Null
Push-Location $project
try {
    $metadata = cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read dependency license metadata.' }
    $notices = @('# ICE 0.2.0 / third-party notices', '', 'ICE package license: Apache-2.0. Dependency license texts follow in this directory.', '')
    foreach ($package in $metadata.packages) {
        if ($package.name -eq 'ice') { continue }
        $notices += "- $($package.name) $($package.version): $($package.license)"
        $crateDir = Split-Path -Parent $package.manifest_path
        foreach ($license in Get-ChildItem -LiteralPath $crateDir -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' }) {
            Copy-Item -LiteralPath $license.FullName -Destination (Join-Path $licenses "$($package.name)-$($package.version)-$($license.Name)") -Force
        }
    }
    $notices | Set-Content -LiteralPath (Join-Path $licenses 'NOTICE.md') -Encoding UTF8
} finally { Pop-Location }
$sums = foreach ($file in Get-ChildItem -LiteralPath $build -File -Recurse -Force | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object FullName) {
    $relative = $file.FullName.Substring($build.Length + 1).Replace('\','/')
    "$((Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant())  $relative"
}
$sums | Set-Content -LiteralPath (Join-Path $build 'SHA256SUMS.txt') -Encoding ASCII
Write-Host "Distribution prepared: $release"

