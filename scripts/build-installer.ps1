[CmdletBinding()]
param(
    [string]$NsisCompiler = $env:MAKENSIS,
    [string]$CrtDirectory,
    [switch]$TestPackage
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repository = Split-Path $PSScriptRoot -Parent

function Assert-X64Binary([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) { throw "Invalid Windows binary: $Path" }
        $stream.Position = 0x3c
        $stream.Position = $reader.ReadUInt32()
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
            throw "The installer requires an x64 Windows binary: $Path"
        }
    }
    finally { $reader.Dispose() }
}

if (!$NsisCompiler) {
    $command = Get-Command makensis.exe -ErrorAction SilentlyContinue
    if ($command) { $NsisCompiler = $command.Source }
}
if (!$NsisCompiler) {
    $candidates = @(
        "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
        "$env:ProgramFiles\NSIS\makensis.exe"
        "$env:LOCALAPPDATA\Programs\NSIS\makensis.exe"
    )
    # An existing Electron build cache can supply the compiler; Node is not used.
    $cache = Join-Path $env:LOCALAPPDATA 'electron-builder\Cache'
    if (Test-Path -LiteralPath $cache) {
        $candidates += @(Get-ChildItem -Path "$cache\nsis-*\*\makensis.exe" -File -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending | ForEach-Object FullName)
    }
    $NsisCompiler = $candidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if (!$NsisCompiler -or !(Test-Path -LiteralPath $NsisCompiler -PathType Leaf)) {
    throw 'Install NSIS 3 from https://nsis.sourceforge.io/Download, or pass -NsisCompiler C:\path\to\makensis.exe.'
}
$NsisCompiler = (Resolve-Path -LiteralPath $NsisCompiler).Path

if (!$CrtDirectory) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vswhere) {
        $locations = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'VC\Redist\MSVC\**\x64\Microsoft.VC*.CRT\vcruntime140.dll')
        if ($LASTEXITCODE -ne 0) { throw 'Could not locate the Visual C++ redistributable files.' }
        if ($locations.Count) { $CrtDirectory = Split-Path ($locations | Sort-Object -Descending | Select-Object -First 1) -Parent }
    }
}
if (!$CrtDirectory -or !(Test-Path -LiteralPath (Join-Path $CrtDirectory 'vcruntime140.dll'))) {
    throw 'Install the MSVC C++ x64 build tools, or pass -CrtDirectory pointing to the x64 Microsoft.VC*.CRT redistributable folder.'
}

Push-Location $repository
try {
    $metadataLines = & cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $metadata = ($metadataLines -join "`n") | ConvertFrom-Json
    $package = $metadata.packages | Where-Object name -eq 'presence-desktop'
    $version = $package.version
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw 'The Windows installer requires a three-part numeric Cargo version.' }

    $buildLines = & cargo build --locked --release -p presence-desktop --message-format=json
    if ($LASTEXITCODE -ne 0) { throw 'The release build failed.' }
    $executable = $null
    foreach ($line in $buildLines) {
        $message = $line | ConvertFrom-Json
        if ($message.reason -eq 'compiler-artifact' -and $message.target.name -eq 'presence-control' -and $message.executable) {
            $executable = $message.executable
        }
    }
    if (!$executable) { throw 'Cargo did not report the release executable.' }
    Assert-X64Binary $executable

    $release = Join-Path $repository 'release'
    $stage = Join-Path $repository ('artifacts\installer\' + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $release, $stage -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $stage 'presence-control.exe')
    $runtime = @(Get-ChildItem -LiteralPath $CrtDirectory -Filter '*.dll' -File)
    foreach ($dll in $runtime) {
        Assert-X64Binary $dll.FullName
        Copy-Item -LiteralPath $dll.FullName -Destination $stage
    }
    Copy-Item -LiteralPath 'packaging\windows\README.txt', 'packaging\windows\THIRD-PARTY-NOTICES.txt' -Destination $stage
    Copy-Item -LiteralPath 'vendor\i-slint-renderer-software\LICENSES\LicenseRef-Slint-Royalty-free-2.0.md' -Destination (Join-Path $stage 'Slint-LICENSE.md')

    # Generate explicit install/delete lists from this payload. Uninstall never
    # uses wildcards, so additional files in a chosen directory survive removal.
    $payloadFiles = @(Get-ChildItem -LiteralPath $stage -File | Sort-Object Name)
    $manifest = @('!macro InstallPayload')
    foreach ($file in $payloadFiles) { $manifest += '  File "${PAYLOAD_DIR}\' + $file.Name + '"' }
    $manifest += '!macroend', '!macro UninstallPayload'
    foreach ($file in $payloadFiles) { $manifest += '  Delete "$INSTDIR\' + $file.Name + '"' }
    $manifest += '!macroend'
    [IO.File]::WriteAllLines((Join-Path $stage 'payload.nsh'), $manifest)

    $installer = Join-Path $release "Presence-Control-$version-windows-x64-setup.exe"
    $appKey = 'Software\Presence Control\Rust Slint'
    $uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\PresenceControl.RustSlint'
    $shortcut = 'Presence Control (Rust)'
    $defines = @(
        "/DAPP_VERSION=$version"
        "/DPAYLOAD_DIR=$stage"
        "/DAPP_ICON=$(Join-Path $repository 'assets\icon.ico')"
        "/DINSTALLER_FILE=$installer"
    )
    if ($TestPackage) {
        # Same installer source and payload; separate identity so lifecycle
        # verification cannot upgrade or uninstall the user's real application.
        $identity = [guid]::NewGuid().ToString('N')
        $installer = Join-Path $stage 'installer-test-setup.exe'
        $appKey = "Software\Presence Control\Installer Tests\$identity"
        $uninstallKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\PresenceControl.Test.$identity"
        $shortcut = "Presence Control (Installer Test $identity)"
        $defines = @($defines | Where-Object { !($_.StartsWith('/DINSTALLER_FILE=')) })
        $defines += "/DINSTALLER_FILE=$installer", "/DAPP_KEY=$appKey", "/DUNINSTALL_KEY=$uninstallKey", "/DSHORTCUT=$shortcut", "/DSETUP_MUTEX=Local\PresenceControlInstallerTest.$identity"
        $defines += '/DDEFAULT_INSTALL_DIR=$LOCALAPPDATA\Programs\Presence Control Installer Test ' + $identity
    }
    & $NsisCompiler /V2 /WX @defines (Join-Path $repository 'packaging\windows\installer.nsi')
    if ($LASTEXITCODE -ne 0) { throw 'NSIS installer compilation failed.' }

    if (!$TestPackage) {
        # Keep the portable binary alongside setup. Its runtime is app-local too.
        foreach ($file in $payloadFiles) { Copy-Item -LiteralPath $file.FullName -Destination $release -Force }
        $distribution = @($payloadFiles.Name) + (Split-Path $installer -Leaf)
        $checksums = foreach ($name in $distribution) {
            $hash = (Get-FileHash -LiteralPath (Join-Path $release $name) -Algorithm SHA256).Hash.ToLowerInvariant()
            "$hash  $name"
        }
        [IO.File]::WriteAllLines((Join-Path $release 'SHA256SUMS.txt'), [string[]]$checksums)
    }
    Write-Host "Installer: $installer"
    Write-Host "Payload: $stage"
    Write-Host "Compiler: $NsisCompiler"
    [pscustomobject]@{ InstallerPath = $installer; AppKey = "HKCU:\$appKey"; UninstallKey = "HKCU:\$uninstallKey"; Shortcut = $shortcut; TestPackage = [bool]$TestPackage }
}
finally { Pop-Location }
