[CmdletBinding()]
param([string]$ProbePath, [string]$ReportPath)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repository = Split-Path $PSScriptRoot -Parent
if (!$ReportPath) {
    $ReportPath = Join-Path $repository ('artifacts\repaint\' + [guid]::NewGuid().ToString() + '.json')
}
if (!$ProbePath) {
    Push-Location $repository
    try {
        $lines = & cargo build --locked --release -p presence-desktop --example repaint_probe --message-format=json
        if ($LASTEXITCODE -ne 0) { throw 'Could not build the Windows repaint probe.' }
        foreach ($line in $lines) {
            $message = $line | ConvertFrom-Json
            if ($message.reason -eq 'compiler-artifact' -and $message.target.name -eq 'repaint_probe' -and $message.executable) {
                $ProbePath = $message.executable
            }
        }
    }
    finally { Pop-Location }
}
if (!$ProbePath) { throw 'Cargo did not report the repaint probe executable.' }
$ProbePath = (Resolve-Path -LiteralPath $ProbePath).Path
$results = foreach ($scale in '1', '1.25', '1.5') {
    $start = [Diagnostics.ProcessStartInfo]::new($ProbePath)
    $start.UseShellExecute = $false
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    # Environment changes belong only to the isolated child, never to the
    # user's running application or Windows settings.
    $start.Environment['SLINT_SCALE_FACTOR'] = $scale
    $start.Environment['SLINT_BACKEND'] = 'winit-software'
    $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $deadline = [datetime]::UtcNow.AddSeconds(20)
    while (!$process.WaitForExit(250)) {
        if ([datetime]::UtcNow -gt $deadline) {
            $process.Kill()
            throw "Repaint probe timed out at scale $scale."
        }
    }
    $report = $stdout.GetAwaiter().GetResult() | ConvertFrom-Json
    [ordered]@{
        scale = $scale
        exitCode = $process.ExitCode
        passed = ($process.ExitCode -eq 0 -and $report.passed)
        checks = @($report.checks)
        error = $report.error
        actualScale = $report.scaleFactor
        stderr = $stderr.GetAwaiter().GetResult().Trim()
    }
}
$passed = @($results | Where-Object { !$_.passed }).Count -eq 0
$output = [ordered]@{ passed = $passed; probe = $ProbePath; results = @($results) }
New-Item -ItemType Directory -Path (Split-Path $ReportPath -Parent) -Force | Out-Null
$output | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $ReportPath -Encoding utf8
Write-Host "Repaint report: $ReportPath"
if (!$passed) { throw 'The native Windows repaint regression failed. See the report.' }
