$ErrorActionPreference = "Stop"
$skillBinary = Join-Path (Split-Path -Parent $PSScriptRoot) "bin\browser-cli.exe"
if (Test-Path $skillBinary) { & $skillBinary doctor; exit $LASTEXITCODE }
Write-Output '{"ok":false,"error":"command_not_found","message":"Skill-local browser-cli.exe is missing. Run scripts/bootstrap.ps1 first."}'
exit 1
