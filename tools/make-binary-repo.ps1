[CmdletBinding()]
param([string]$Output, [string]$Version = '0.2.0')
$ErrorActionPreference = 'Stop'
$source = Split-Path -Parent $PSScriptRoot
if (-not $Output) { $Output = Join-Path $source 'dist\ice-binaries' }
$out = [IO.Path]::GetFullPath($Output).TrimEnd('\')
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Version must be X.Y.Z.' }
if ($out -eq $source -or $source.StartsWith($out + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Output must not be the source directory or its ancestor.' }
if ((Test-Path -LiteralPath $out) -and @(Get-ChildItem -LiteralPath $out -Force).Count -gt 0 -and -not (Test-Path -LiteralPath (Join-Path $out 'DISTRIBUTION.json'))) {
    throw 'Refusing to overwrite a directory that is not an ICE binary distribution.'
}
$payloads = @(
    @{target='windows-x86_64';file='ice.exe';input='target\release\ice.exe';label='Windows x64';status='Executed and installed on Windows'},
    @{target='linux-x86_64';file='ice';input='target\x86_64-unknown-linux-musl\release\ice';label='Linux x64';status='Cross-compiled; native execution pending'},
    @{target='linux-aarch64';file='ice';input='target\aarch64-unknown-linux-musl\release\ice';label='Linux ARM64';status='Cross-compiled; native execution pending'},
    @{target='macos-x86_64';file='ice';input='target\x86_64-apple-darwin\release\ice';label='macOS Intel';status='Cross-compiled; native execution pending'},
    @{target='macos-aarch64';file='ice';input='target\aarch64-apple-darwin\release\ice';label='macOS Apple Silicon';status='Cross-compiled; native execution pending'}
)
foreach ($payload in $payloads) { if (-not (Test-Path -LiteralPath (Join-Path $source $payload.input))) { throw "Build missing: $($payload.input)" } }
$utf8 = [Text.UTF8Encoding]::new($false)
function Write-ReleaseText([string]$Relative, [string]$Text) {
    $path = Join-Path $out $Relative
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
    [IO.File]::WriteAllText($path, $Text.Replace("`r`n", "`n"), $utf8)
}
New-Item -ItemType Directory -Force -Path $out | Out-Null
$template = Join-Path $PSScriptRoot 'binary-repo'
foreach ($relative in @('install.ps1','install.sh','.gitattributes','docs\WEBSITE.md','.github\workflows\verify-install.yml')) {
    Write-ReleaseText $relative ([IO.File]::ReadAllText((Join-Path $template $relative)))
}
$sh = [IO.File]::ReadAllText((Join-Path $template 'install.sh')).Replace("`r`n", "`n")
Write-ReleaseText 'bash/install.sh' ($sh.Replace('#!/usr/bin/env bash', "#!/usr/bin/env bash`nICE_EXPECT_OS=Linux"))
Write-ReleaseText 'mac/install.sh' ($sh.Replace('#!/usr/bin/env bash', "#!/usr/bin/env bash`nICE_EXPECT_OS=Darwin"))
Write-ReleaseText 'windows/install.ps1' ([IO.File]::ReadAllText((Join-Path $template 'install.ps1')))
# Compatibility names carry the same complete installers, without another network hop.
Write-ReleaseText 'download.sh' $sh
Write-ReleaseText 'download.ps1' ([IO.File]::ReadAllText((Join-Path $template 'install.ps1')))
New-Item -ItemType Directory -Force -Path (Join-Path $out 'assets\icons') | Out-Null
foreach ($file in Get-ChildItem -LiteralPath (Join-Path $template 'assets\icons') -File) {
    $content = [IO.File]::ReadAllText($file.FullName).Replace('currentColor','#50d2ff')
    Write-ReleaseText "assets/icons/$($file.Name)" $content
}
Copy-Item -LiteralPath (Join-Path $template 'assets\hero.svg') -Destination (Join-Path $out 'assets\hero.svg') -Force
foreach ($file in @('ice-preview.gif','welcome.png','session.png','connect.png','floe.svg')) {
    Copy-Item -LiteralPath (Join-Path $source "brand\$file") -Destination (Join-Path $out "assets\$file") -Force
}
$sums = @(); $table = @('| Platform | Download | Verification |', '| --- | --- | --- |'); $manifest = @()
foreach ($payload in $payloads) {
    $inputFile = Join-Path $source $payload.input
    $relative = "$($payload.target)/$($payload.file)"
    $destination = Join-Path $out "releases\$Version\$relative"
    $hash = (Get-FileHash -LiteralPath $inputFile -Algorithm SHA256).Hash.ToLowerInvariant()
    if ((Test-Path -LiteralPath $destination) -and (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -ine $hash) {
        throw "Release $Version already has different bytes for $relative. Publish a new version; released binaries are immutable."
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    Copy-Item -LiteralPath $inputFile -Destination $destination -Force
    $sums += "$hash  $relative"
    $table += "| $($payload.label) | [Download](https://raw.githubusercontent.com/loayabdalslam/ICE/main/releases/$Version/$relative) | $($payload.status) |"
    $manifest += @{target=$payload.target;path="releases/$Version/$relative";sha256=$hash;bytes=(Get-Item -LiteralPath $inputFile).Length;verification=$payload.status}
}
Write-ReleaseText "releases/$Version/SHA256SUMS.txt" (($sums -join "`n") + "`n")
Write-ReleaseText 'LATEST' "$Version`n"
Write-ReleaseText 'DISTRIBUTION.json' ((@{schema=1;name='ICE';version=$Version;repository='loayabdalslam/ICE';sourceIncluded=$false;artifacts=$manifest} | ConvertTo-Json -Depth 5) + "`n")
Write-ReleaseText 'README.md' ([IO.File]::ReadAllText((Join-Path $template 'README.md')).Replace('@PLATFORM_ROWS@', ($table -join "`n")))
Push-Location $source
try {
    $metadata = cargo metadata --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed.' }
    $notices = @('# Third-party notices', '', 'ICE declares Apache-2.0. Dependency license texts are included below.', '')
    foreach ($package in $metadata.packages) {
        if ($package.name -eq 'ice') { continue }
        $notices += "- $($package.name) $($package.version): $($package.license)"
        foreach ($license in Get-ChildItem -LiteralPath (Split-Path -Parent $package.manifest_path) -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)' }) {
            Write-ReleaseText "licenses/$($package.name)-$($package.version)-$($license.Name)" ([IO.File]::ReadAllText($license.FullName))
            if ($package.name -eq 'anyhow' -and $license.Name -eq 'LICENSE-APACHE') { Write-ReleaseText 'LICENSE' ([IO.File]::ReadAllText($license.FullName)) }
        }
    }
    Write-ReleaseText 'licenses/README.md' (($notices -join "`n") + "`n")
} finally { Pop-Location }
if (-not (Test-Path -LiteralPath (Join-Path $out '.git'))) { git init -b main $out; if ($LASTEXITCODE -ne 0) { throw 'git init failed' } }
Write-Host "Prepared binary distribution: $out" -ForegroundColor Cyan
Write-Host 'No directories were deleted. Existing Git history and released binaries are preserved.'
