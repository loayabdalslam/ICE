# ICE installer for Windows (PowerShell 5.1+ and PowerShell 7).
#
# Default: clone the ICE repository, build it with cargo, and install ice.exe.
# If git/cargo/the MSVC build tools aren't available, falls back to the
# prebuilt, SHA-256-verified release binary. Re-run (or `ice update`) to update.
#
#   irm https://raw.githubusercontent.com/loayabdalslam/ICE/main/install.ps1 | iex
[CmdletBinding()]
param(
    [string]$Ref = $(if ($env:ICE_REF) { $env:ICE_REF } else { 'main' }),
    [string]$Repo = $(if ($env:ICE_REPO) { $env:ICE_REPO } else { 'https://github.com/loayabdalslam/ICE.git' }),
    [string]$SourceDir = $env:ICE_SOURCE_DIR,
    [string]$InstallDir = $env:ICE_INSTALL_DIR,
    [string]$Version = $env:ICE_VERSION,
    [string]$BaseUrl = $env:ICE_BASE_URL,
    [switch]$Binary,
    [switch]$NoRustup,
    [switch]$NoPath,
    [switch]$Uninstall
)
& {
    $ErrorActionPreference = 'Stop'
    if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\ICE' }
    if (-not $SourceDir) { $SourceDir = Join-Path $env:LOCALAPPDATA 'ICE\src' }
    if (-not $BaseUrl) { $BaseUrl = 'https://raw.githubusercontent.com/loayabdalslam/ICE/main' }
    $configDir = if ($env:ICE_CONFIG_DIR) { $env:ICE_CONFIG_DIR } else { Join-Path $HOME '.ice' }
    $destination = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')
    $binaryPath = Join-Path $destination 'ice.exe'
    $recordPath = Join-Path $destination '.ice-install.json'

    if ($Uninstall) {
        if (Test-Path -LiteralPath $binaryPath) { Remove-Item -LiteralPath $binaryPath }
        $record = if (Test-Path -LiteralPath $recordPath) { try { Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json } catch { $null } } else { $null }
        if ($record -and $record.pathAdded) {
            $userPath = [string][Environment]::GetEnvironmentVariable('Path', 'User')
            $remaining = @($userPath -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $destination }) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $remaining, 'User')
        }
        if (Test-Path -LiteralPath $recordPath) { Remove-Item -LiteralPath $recordPath }
        Write-Host 'ICE removed. Your settings in ~/.ice and the source checkout were kept.' -ForegroundColor Cyan
        return
    }
    if (-not [Environment]::Is64BitOperatingSystem) { throw 'ICE requires 64-bit Windows.' }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Write-Host "`n  ■ ICE  Intent. Compile. Execute.`n" -ForegroundColor Cyan

    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('ice-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $temporary | Out-Null
    $built = $null
    $method = 'binary'
    try {
        if (-not $Binary -and -not $Version) {
            try {
                if (-not (Get-Command git.exe -ErrorAction SilentlyContinue)) { throw 'git is not installed (https://git-scm.com/download/win).' }
                Write-Host "  Source   $Repo ($Ref)`n  Into     $SourceDir`n"
                if (Test-Path -LiteralPath (Join-Path $SourceDir '.git')) {
                    git -C $SourceDir remote set-url origin $Repo
                    git -C $SourceDir fetch --quiet --depth 1 origin $Ref
                    if ($LASTEXITCODE -ne 0) { throw "git fetch failed" }
                    git -C $SourceDir checkout --quiet --force FETCH_HEAD
                } else {
                    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $SourceDir) | Out-Null
                    git clone --quiet --depth 1 --branch $Ref $Repo $SourceDir
                    if ($LASTEXITCODE -ne 0) { throw "git clone failed" }
                }
                $cargoHome = Join-Path $HOME '.cargo\bin'
                if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue) -and (Test-Path (Join-Path $cargoHome 'cargo.exe'))) { $env:Path = "$cargoHome;$env:Path" }
                if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
                    if ($NoRustup) { throw 'cargo is not installed (https://rustup.rs).' }
                    Write-Host '  Rust     not found · installing the minimal toolchain with rustup…'
                    $rustup = Join-Path $temporary 'rustup-init.exe'
                    Invoke-WebRequest -UseBasicParsing -Uri 'https://win.rustup.rs/x86_64' -OutFile $rustup -TimeoutSec 300
                    & $rustup -y --profile minimal --no-modify-path | Out-Null
                    if ($LASTEXITCODE -ne 0) { throw 'rustup failed.' }
                    $env:Path = "$cargoHome;$env:Path"
                }
                Write-Host '  Build    cargo build --release — the first build takes a few minutes…'
                Push-Location $SourceDir
                try { cargo build --release --locked --quiet } finally { Pop-Location }
                if ($LASTEXITCODE -ne 0) { throw 'cargo build failed (the MSVC C++ build tools may be missing: https://aka.ms/vs/17/release/vs_BuildTools.exe).' }
                $built = Join-Path $SourceDir 'target\release\ice.exe'
                $method = 'source'
            } catch {
                Write-Host "  Building from source isn't possible here: $($_.Exception.Message)" -ForegroundColor Yellow
                Write-Host '  Falling back to the prebuilt release binary.' -ForegroundColor Yellow
            }
        }

        if (-not $built) {
            $baseUri = [Uri]$BaseUrl
            if ($baseUri.Scheme -ne 'https' -and -not ($baseUri.Scheme -eq 'http' -and $baseUri.IsLoopback)) { throw 'ICE_BASE_URL must use HTTPS.' }
            $BaseUrl = $BaseUrl.TrimEnd('/')
            if (-not $Version) {
                $latestFile = Join-Path $temporary 'LATEST'
                Invoke-WebRequest -UseBasicParsing -Uri "$BaseUrl/LATEST" -OutFile $latestFile -TimeoutSec 60
                $Version = [IO.File]::ReadAllText($latestFile).Trim()
            }
            if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Invalid release version: $Version" }
            $release = "$BaseUrl/releases/$Version"
            $asset = 'windows-x86_64/ice.exe'
            Write-Host "  Release  $Version for Windows x64"
            $sums = Join-Path $temporary 'SHA256SUMS.txt'
            Invoke-WebRequest -UseBasicParsing -Uri "$release/SHA256SUMS.txt" -OutFile $sums -TimeoutSec 120
            $entries = @(Get-Content -LiteralPath $sums | Where-Object { $_ -match ('^[a-fA-F0-9]{64}  ' + [regex]::Escape($asset) + '$') })
            if ($entries.Count -ne 1) { throw "No verified binary for $asset in ICE $Version." }
            $expected = $entries[0].Substring(0, 64)
            $download = Join-Path $temporary 'ice.exe'
            Invoke-WebRequest -UseBasicParsing -Uri "$release/$asset" -OutFile $download -TimeoutSec 300
            if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ine $expected) { throw 'SHA-256 mismatch. Installation stopped.' }
            $built = $download
        }

        & $built --version | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'The binary cannot run on this machine. Your installation was not changed.' }
        $previous = if (Test-Path -LiteralPath $recordPath) { try { Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json } catch { $null } } else { $null }
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        $stage = Join-Path $destination ('.ice-new-' + [guid]::NewGuid().ToString('N') + '.exe')
        try {
            Copy-Item -LiteralPath $built -Destination $stage
            if (Test-Path -LiteralPath $binaryPath) {
                # A running ice.exe can be renamed away but not overwritten.
                $old = Join-Path $destination ('.ice-old-' + [guid]::NewGuid().ToString('N') + '.exe')
                Move-Item -LiteralPath $binaryPath -Destination $old -Force
                Remove-Item -LiteralPath $old -ErrorAction SilentlyContinue
            }
            Move-Item -LiteralPath $stage -Destination $binaryPath -Force
        } finally { if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage } }

        $pathAdded = [bool]($previous -and $previous.pathAdded)
        if (-not $NoPath) {
            $userPath = [string][Environment]::GetEnvironmentVariable('Path', 'User')
            if (@($userPath -split ';' | Where-Object { $_.TrimEnd('\') -ieq $destination }).Count -eq 0) {
                [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $destination).TrimStart(';'), 'User')
                $pathAdded = $true
            }
            if (@($env:Path -split ';' | Where-Object { $_.TrimEnd('\') -ieq $destination }).Count -eq 0) { $env:Path += ';' + $destination }
        }
        @{ directory = $destination; method = $method; pathAdded = $pathAdded } | ConvertTo-Json | Set-Content -LiteralPath $recordPath -Encoding UTF8
        New-Item -ItemType Directory -Force -Path $configDir | Out-Null
        $install = @{ method = $method; bin = $binaryPath; ref = $Ref }
        if ($method -eq 'source') { $install.source_dir = $SourceDir } else { $install.version = $Version }
        $install | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $configDir 'install.json') -Encoding UTF8

        $ver = & $binaryPath --version
        Write-Host "`n  ✔ Installed $ver → $binaryPath ($method)" -ForegroundColor Green
        Write-Host "`n  Get started:`n    cd your-project`n    ice`n`n  Update any time with:  ice update"
        if (-not $NoPath) { Write-Host '  Open a new terminal if ice is not found yet.' }
        if (-not (Get-Command bash.exe -ErrorAction SilentlyContinue)) { Write-Host '  Tip: install Git for Windows so ICE can run shell commands through bash.exe.' -ForegroundColor Yellow }
    } finally {
        $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        $resolved = [IO.Path]::GetFullPath($temporary)
        if ($resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Split-Path -Leaf $resolved) -match '^ice-install-[a-f0-9]{32}$') {
            Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
