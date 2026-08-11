$ErrorActionPreference = "Stop"
$skillBinary = Join-Path (Split-Path -Parent $PSScriptRoot) "bin\browser-cli.exe"
if (Test-Path $skillBinary) { & $skillBinary doctor; exit $LASTEXITCODE }
$command = Get-Command browser-cli -ErrorAction SilentlyContinue
if (-not $command) { Write-Output '{"ok":false,"error":"command_not_found","message":"Run bootstrap.ps1 first."}'; exit 1 }
& browser-cli doctor
exit $LASTEXITCODE
