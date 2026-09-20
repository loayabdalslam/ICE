<#
ICE / FLOE Edition — Windows x64 installer
  .\install.ps1                         Install for the current user and add to PATH
  .\install.ps1 -InstallDir C:\Tools\ICE -NoPath -NoShortcut
  .\install.ps1 -Uninstall              Remove this install and its PATH entry
No administrator rights or Rust toolchain required. Install from the complete repo.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\ICE'),
    [switch]$NoPath,
    [switch]$NoShortcut,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$destination = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')
$marker = Join-Path $destination '.ice-install.json'
$shortcut = Join-Path ([Environment]::GetFolderPath('StartMenu')) 'Programs\ICE.lnk'
if ($Uninstall) {
    if (-not (Test-Path -LiteralPath $marker)) { throw 'This directory is not an ICE-managed installation.' }
    $record = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
    if ($record.directory -ne $destination) { throw 'Installation directory does not match its recorded location.' }
    # Remove only installer-owned files, never recursively delete a user directory.
    foreach ($file in $record.files) {
        $candidate = [IO.Path]::GetFullPath((Join-Path $destination $file))
        if (-not $candidate.StartsWith($destination + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe install manifest path.' }
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { Remove-Item -LiteralPath $candidate }
    }
    if ($record.pathAdded) {
        $current = [string][Environment]::GetEnvironmentVariable('Path', 'User')
        $remaining = @($current -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $destination }) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $remaining, 'User')
        $env:Path = (@($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ine $destination }) -join ';')
    }
    if ($record.shortcut -and (Test-Path -LiteralPath $shortcut)) {
        $link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
        if ($link.TargetPath -ieq (Join-Path $destination 'ice.exe')) { Remove-Item -LiteralPath $shortcut }
    }
    Remove-Item -LiteralPath $marker
    Write-Host 'ICE uninstalled. Workspace and provider configuration were preserved.' -ForegroundColor Cyan
    return
}
if (-not [Environment]::Is64BitOperatingSystem) { throw 'This release requires Windows x64.' }
if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { Write-Host 'Using the Windows x64 build through Windows emulation.' }
$source = Join-Path $PSScriptRoot 'build'
$binary = Join-Path $source 'ice.exe'
$checksum = Join-Path $source 'SHA256SUMS.txt'
if (-not (Test-Path -LiteralPath $binary)) { throw 'Missing build\ice.exe. Keep install.ps1 next to the build directory.' }
if (-not (Test-Path -LiteralPath $checksum)) { throw 'Missing release checksums.' }
foreach ($line in Get-Content -LiteralPath $checksum) {
    if ($line -notmatch '^([a-fA-F0-9]{64})  (.+)$') { throw 'Malformed release checksum.' }
    $expected = $Matches[1]; $relative = $Matches[2]
    $checked = [IO.Path]::GetFullPath((Join-Path $source $relative))
    $sourceRoot = [IO.Path]::GetFullPath($source).TrimEnd('\')
    if (-not $checked.StartsWith($sourceRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe release path.' }
    if ((Get-FileHash -LiteralPath $checked -Algorithm SHA256).Hash -ine $expected) { throw "Checksum mismatch: $relative" }
}
if ((Test-Path -LiteralPath $destination) -and -not (Test-Path -LiteralPath $marker)) {
    if (@(Get-ChildItem -LiteralPath $destination -Force).Count -gt 0) { throw 'Choose an empty directory or an existing ICE-managed installation.' }
}
$old = if (Test-Path -LiteralPath $marker) { Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json } else { $null }
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$owned = @()
foreach ($file in Get-ChildItem -LiteralPath $source -File -Recurse -Force) {
    $relative = $file.FullName.Substring($sourceRoot.Length + 1)
    $target = Join-Path $destination $relative
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
    Copy-Item -LiteralPath $file.FullName -Destination $target -Force
    $owned += $relative
}
$pathAdded = [bool]($old -and $old.pathAdded)
if (-not $NoPath) {
    $current = [string][Environment]::GetEnvironmentVariable('Path', 'User')
    if (@($current -split ';' | Where-Object { $_.TrimEnd('\') -ieq $destination }).Count -eq 0) {
        [Environment]::SetEnvironmentVariable('Path', (($current.TrimEnd(';') + ';' + $destination).TrimStart(';')), 'User')
        $pathAdded = $true
    }
    if (@($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $destination }).Count -eq 0) { $env:Path += ';' + $destination }
}
$madeShortcut = [bool]($old -and $old.shortcut)
if (-not $NoShortcut) {
    $link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
    $link.TargetPath = Join-Path $destination 'ice.exe'
    $link.WorkingDirectory = [Environment]::GetFolderPath('UserProfile')
    $link.IconLocation = (Join-Path $destination 'brand\ice.ico') + ',0'
    $link.Description = 'ICE - Intent. Compile. Execute.'
    $link.Save()
    $madeShortcut = $true
}
@{directory=$destination;files=$owned;pathAdded=$pathAdded;shortcut=$madeShortcut;version='0.2.0'} | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $marker -Encoding UTF8
& (Join-Path $destination 'ice.exe') --version
if ($LASTEXITCODE -ne 0) { throw 'Installed executable failed its version check.' }
Write-Host "ICE is ready at $destination" -ForegroundColor Cyan
Write-Host 'Open a new terminal and run ice. Use ice --demo for an offline session.'
if (-not (Get-Command bash.exe -ErrorAction SilentlyContinue)) { Write-Host 'Shell actions need Git for Windows (bash.exe on PATH). The TUI itself is ready.' }
