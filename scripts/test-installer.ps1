[CmdletBinding()]
param([string]$InstallerPath, [switch]$Isolated)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repository = Split-Path $PSScriptRoot -Parent
$testPackage = $null
if ($Isolated) {
    if ($InstallerPath) { throw '-Isolated builds its own package; do not combine it with -InstallerPath.' }
    $testPackage = & (Join-Path $PSScriptRoot 'build-installer.ps1') -TestPackage
    $InstallerPath = $testPackage.InstallerPath
}
if (!$InstallerPath) {
    $InstallerPath = Get-ChildItem -LiteralPath (Join-Path $repository 'release') -Filter '*-windows-x64-setup.exe' |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (!$InstallerPath) { throw 'Build the installer first with scripts\build-installer.ps1.' }
$InstallerPath = (Resolve-Path -LiteralPath $InstallerPath).Path
$appKey = 'HKCU:\Software\Presence Control\Rust Slint'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\PresenceControl.RustSlint'
$startShortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Presence Control (Rust).lnk'
$desktopShortcut = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'Presence Control (Rust).lnk'
if ($testPackage) {
    $appKey = $testPackage.AppKey
    $uninstallKey = $testPackage.UninstallKey
    $startShortcut = Join-Path ([Environment]::GetFolderPath('Programs')) ($testPackage.Shortcut + '.lnk')
    $desktopShortcut = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) ($testPackage.Shortcut + '.lnk')
}
if ((Test-Path $appKey) -or (Test-Path $uninstallKey) -or (Test-Path -LiteralPath $startShortcut) -or (Test-Path -LiteralPath $desktopShortcut)) {
    throw 'An existing Rust installation or shortcut was found. Use -Isolated or run the test in a clean Windows account.'
}

$run = Join-Path $repository ('artifacts\installer-test\' + [guid]::NewGuid().ToString())
# Spaces and Unicode exercise the NSIS /D and _?= command-line conventions.
$installDirectory = Join-Path $run 'Presence Control é'
New-Item -ItemType Directory -Path $run -Force | Out-Null
$checks = [Collections.Generic.List[string]]::new()
$installed = $false
$lock = $null

function Check([bool]$Condition, [string]$Description) {
    if (!$Condition) { throw $Description }
    $checks.Add($Description)
}

function Run-Setup([string]$Executable, [string]$Arguments, [int]$ExpectedCode = 0) {
    $process = Start-Process -FilePath $Executable -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    $deadline = [datetime]::UtcNow.AddSeconds(60)
    while (!$process.WaitForExit(500)) {
        if ([datetime]::UtcNow -gt $deadline) { throw "Setup did not finish: $Executable" }
    }
    if ($process.ExitCode -ne $ExpectedCode) {
        throw "Setup returned $($process.ExitCode), expected ${ExpectedCode}: $Executable"
    }
}

function Data-Fingerprint {
    $data = Join-Path $env:APPDATA 'dev.presence.control.rust.slint'
    $hashes = foreach ($name in 'config.json', 'usage.json', 'archive.json') {
        $path = Join-Path $data $name
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            "$name=$((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash)"
        }
        else { "$name=absent" }
    }
    $hashes -join ';'
}

$originalData = Data-Fingerprint
$failure = $null
try {
    Run-Setup $InstallerPath "/S /D=$installDirectory"
    $installed = $true
    $exe = Join-Path $installDirectory 'presence-control.exe'
    $uninstaller = Join-Path $installDirectory 'Uninstall.exe'
    Check (Test-Path -LiteralPath $exe) 'Silent install creates the executable in a path containing spaces and Unicode.'
    Check ((Get-FileHash -LiteralPath $exe).Hash -eq (Get-FileHash -LiteralPath (Join-Path $repository 'release\presence-control.exe')).Hash) 'Installed executable matches the release build.'
    $registration = Get-ItemProperty $uninstallKey
    Check ($registration.InstallLocation -eq $installDirectory -and $registration.DisplayVersion -eq (Get-Item $InstallerPath).VersionInfo.ProductVersion) 'Windows Installed apps registration contains the correct directory and version.'
    Check ($registration.UninstallString -eq "`"$uninstaller`"" -and $registration.QuietUninstallString -eq "`"$uninstaller`" /S") 'Interactive and silent uninstall commands quote their paths correctly.'
    $shell = New-Object -ComObject WScript.Shell
    Check ((Test-Path -LiteralPath $startShortcut) -and $shell.CreateShortcut($startShortcut).TargetPath -eq $exe) 'Start menu shortcut targets the installed application.'
    Check (!(Test-Path -LiteralPath $desktopShortcut)) 'Desktop shortcut is optional and is not added during a default silent install.'
    foreach ($dll in Get-ChildItem -LiteralPath (Join-Path $repository 'release') -Filter '*.dll') {
        Check ((Get-FileHash -LiteralPath (Join-Path $installDirectory $dll.Name)).Hash -eq (Get-FileHash -LiteralPath $dll.FullName).Hash) "Bundled runtime matches: $($dll.Name)"
    }

    $testUninstaller = Join-Path $run 'test-uninstall.exe'
    Copy-Item -LiteralPath $uninstaller -Destination $testUninstaller
    $lock = [IO.File]::Open($exe, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    Run-Setup $InstallerPath "/S /D=$installDirectory" 2
    Run-Setup $testUninstaller "/S _?=$installDirectory" 2
    Check ((Test-Path -LiteralPath $exe) -and (Test-Path $uninstallKey)) 'Locked application blocks upgrade and uninstall without removing the installation.'
    $lock.Dispose()
    $lock = $null

    $sentinel = Join-Path $installDirectory 'keep-this-file.txt'
    [IO.File]::WriteAllText($sentinel, 'Additional files must survive setup and uninstall.')
    # Omit /D on the second run: the registered custom path must be reused.
    Run-Setup $InstallerPath '/S'
    Check ([IO.File]::ReadAllText($sentinel) -eq 'Additional files must survive setup and uninstall.') 'Reinstall reuses the registered directory and preserves additional files.'
    Check ((Get-FileHash -LiteralPath $exe).Hash -eq (Get-FileHash -LiteralPath (Join-Path $repository 'release\presence-control.exe')).Hash) 'Reinstall leaves a complete, correct executable.'

    $smokeDirectory = Join-Path $repository 'artifacts\native-smoke'
    $previousRuns = @(Get-ChildItem -LiteralPath $smokeDirectory -Directory -ErrorAction SilentlyContinue | ForEach-Object Name)
    $smoke = Start-Process -FilePath $exe -ArgumentList '--smoke-test' -WorkingDirectory $run -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $run 'app-stdout.log') -RedirectStandardError (Join-Path $run 'app-stderr.log')
    $deadline = [datetime]::UtcNow.AddSeconds(60)
    $localRuntime = $false
    while (!$smoke.WaitForExit(250)) {
        try {
            if (@($smoke.Modules | Where-Object { $_.FileName -eq (Join-Path $installDirectory 'vcruntime140.dll') }).Count -gt 0) { $localRuntime = $true }
        }
        catch { # The process may exit between the wait and module enumeration.
        }
        if ([datetime]::UtcNow -gt $deadline) { throw 'Installed application smoke test timed out.' }
    }
    Check ($smoke.ExitCode -eq 0) 'Installed application completes its isolated native smoke run.'
    Check $localRuntime 'Installed application loads its bundled Visual C++ runtime.'
    $newRuns = @(Get-ChildItem -LiteralPath $smokeDirectory -Directory | Where-Object { $_.Name -notin $previousRuns })
    Check ($newRuns.Count -eq 1) 'Installed smoke run creates one isolated workspace.'
    $smokeReportPath = Join-Path $newRuns[0].FullName 'smoke-report.json'
    $smokeReport = Get-Content -LiteralPath $smokeReportPath -Raw | ConvertFrom-Json
    Check $smokeReport.passed 'All installed native workflow checks pass.'

    Run-Setup $testUninstaller "/S _?=$installDirectory"
    $installed = $false
    Check (!(Test-Path -LiteralPath $exe) -and !(Test-Path -LiteralPath $uninstaller)) 'Uninstall removes the application and its uninstaller.'
    Check (!(Test-Path $appKey) -and !(Test-Path $uninstallKey)) 'Uninstall removes application registration.'
    Check (!(Test-Path -LiteralPath $startShortcut) -and !(Test-Path -LiteralPath $desktopShortcut)) 'Uninstall removes application shortcuts.'
    Check (@(Get-ChildItem -LiteralPath $installDirectory -File).Count -eq 1 -and (Test-Path -LiteralPath $sentinel)) 'Uninstall removes only its payload and keeps additional files.'
    Check ((Data-Fingerprint) -eq $originalData) 'Normal profiles, activity history, and archives are unchanged across the lifecycle.'
}
catch {
    $failure = $_.Exception.Message
    throw
}
finally {
    if ($lock) { $lock.Dispose() }
    if ($installed -and (Test-Path -LiteralPath (Join-Path $installDirectory 'Uninstall.exe'))) {
        # Run a copy outside the install folder so /S can be awaited directly.
        $cleanupUninstaller = Join-Path $run 'cleanup-uninstall.exe'
        Copy-Item -LiteralPath (Join-Path $installDirectory 'Uninstall.exe') -Destination $cleanupUninstaller -Force
        Run-Setup $cleanupUninstaller "/S _?=$installDirectory"
    }
    $report = [ordered]@{ passed = ($null -eq $failure); installer = $InstallerPath; isolated = [bool]$Isolated; directory = $installDirectory; checks = @($checks.ToArray()); error = $failure }
    $report | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $run 'installer-report.json') -Encoding utf8
    Write-Host "Installer report: $(Join-Path $run 'installer-report.json')"
}
