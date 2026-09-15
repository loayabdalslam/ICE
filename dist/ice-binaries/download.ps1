# ICE / binary installer. Windows PowerShell 5.1+ and PowerShell 7.
[CmdletBinding()]
param(
    [string]$Version = $env:ICE_VERSION,
    [string]$InstallDir = $env:ICE_INSTALL_DIR,
    [string]$BaseUrl = $env:ICE_BASE_URL,
    [switch]$NoPath,
    [switch]$DownloadOnly,
    [switch]$Uninstall
)
& {
    $ErrorActionPreference = 'Stop'
    if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\ICE' }
    if (-not $BaseUrl) { $BaseUrl = 'https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main' }
    $destination = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')
    $binary = Join-Path $destination 'ice.exe'
    $recordPath = Join-Path $destination '.ice-install.json'
    if ($Uninstall) {
        if (-not (Test-Path -LiteralPath $recordPath)) { throw 'This directory has no ICE installation record.' }
        $record = Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json
        if ($record.directory -ine $destination) { throw 'Installation record does not match this directory.' }
        if (Test-Path -LiteralPath $binary) { Remove-Item -LiteralPath $binary }
        if ($record.pathAdded) {
            $userPath = [string][Environment]::GetEnvironmentVariable('Path', 'User')
            $remaining = @($userPath -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $destination }) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $remaining, 'User')
            $env:Path = @($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ine $destination }) -join ';'
        }
        Remove-Item -LiteralPath $recordPath
        Write-Host 'ICE removed. Workspace files and provider settings are preserved.' -ForegroundColor Cyan
        return
    }
    if (-not [Environment]::Is64BitOperatingSystem) { throw 'ICE requires 64-bit Windows.' }
    $baseUri = [Uri]$BaseUrl
    if ($baseUri.Scheme -ne 'https' -and -not ($baseUri.Scheme -eq 'http' -and $baseUri.IsLoopback)) {
        throw 'ICE_BASE_URL must use HTTPS (HTTP loopback is allowed for local tests).'
    }
    $BaseUrl = $BaseUrl.TrimEnd('/')
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('ice-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temporary | Out-Null
    try {
        if (-not $Version) {
            $latestFile = Join-Path $temporary 'LATEST'
            Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/LATEST" -OutFile $latestFile -TimeoutSec 60
            $Version = [IO.File]::ReadAllText($latestFile).Trim()
        }
        if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Invalid release version: $Version" }
        $release = "$BaseUrl/releases/$Version"
        $asset = 'windows-x86_64/ice.exe'
        Write-Host "`n  ICE / Intent. Compile. Execute.`n  Installing $Version for Windows x64`n" -ForegroundColor Cyan
        if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64' -or $env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') {
            Write-Host 'This release uses Windows x64 emulation on ARM64.'
        }
        $sums = Join-Path $temporary 'SHA256SUMS.txt'
        Invoke-WebRequest -UseBasicParsing -Uri "$release/SHA256SUMS.txt" -OutFile $sums -TimeoutSec 120
        $entries = @(Get-Content -LiteralPath $sums | Where-Object { $_ -match ('^[a-fA-F0-9]{64}  ' + [regex]::Escape($asset) + '$') })
        if ($entries.Count -ne 1) { throw "No unique verified binary for $asset in ICE $Version." }
        $expected = $entries[0].Substring(0,64)
        $download = Join-Path $temporary 'ice.exe'
        Invoke-WebRequest -UseBasicParsing -Uri "$release/$asset" -OutFile $download -TimeoutSec 180
        if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ine $expected) { throw 'SHA-256 mismatch. Installation stopped.' }
        if (-not $DownloadOnly) {
            & $download --version
            if ($LASTEXITCODE -ne 0) { throw 'The downloaded binary cannot run on this machine. Your installation was not changed.' }
        }
        $previous = if (Test-Path -LiteralPath $recordPath) { Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json } else { $null }
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        $stage = Join-Path $destination ('.ice-new-' + [guid]::NewGuid().ToString('N') + '.exe')
        try {
            Copy-Item -LiteralPath $download -Destination $stage
            Move-Item -LiteralPath $stage -Destination $binary -Force
        } finally { if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage } }
        $pathAdded = [bool]($previous -and $previous.pathAdded)
        if (-not $NoPath -and -not $DownloadOnly) {
            $userPath = [string][Environment]::GetEnvironmentVariable('Path', 'User')
            if (@($userPath -split ';' | Where-Object { $_.TrimEnd('\') -ieq $destination }).Count -eq 0) {
                [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $destination).TrimStart(';'), 'User')
                $pathAdded = $true
            }
            if (@($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $destination }).Count -eq 0) { $env:Path += ';' + $destination }
        }
        @{ directory=$destination; version=$Version; sha256=$expected; pathAdded=$pathAdded } | ConvertTo-Json | Set-Content -LiteralPath $recordPath -Encoding UTF8
        Write-Host "`nInstalled: $binary`nStart:     ice --demo" -ForegroundColor Cyan
        if ($NoPath -or $DownloadOnly) { Write-Host 'PATH was not changed. Run the installed executable by its full path.' }
        else { Write-Host 'Open a new terminal if ice is not available in your current terminal.' }
        if (-not (Get-Command bash.exe -ErrorAction SilentlyContinue)) { Write-Host 'Shell actions require Git for Windows with bash.exe on PATH.' }
    } finally {
        # Delete only the exact unique directory this installer created.
        $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        $resolved = [IO.Path]::GetFullPath($temporary)
        if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -match '^ice-install-[a-f0-9]{32}$') {
            Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
