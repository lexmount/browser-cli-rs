$ErrorActionPreference = "Stop"
$version = if ($env:LEXMOUNT_BROWSER_CLI_VERSION) { $env:LEXMOUNT_BROWSER_CLI_VERSION } else { "0.1.0" }
if (-not [Environment]::Is64BitOperatingSystem) { throw "Only 64-bit Windows is supported" }
$asset = "browser-cli-v$version-x86_64-pc-windows-msvc.zip"
$repo = "https://github.com/lexmount/browser-cli-rs/releases/download/v$version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Invoke-WebRequest -UseBasicParsing "$repo/$asset" -OutFile (Join-Path $tmp $asset)
  Invoke-WebRequest -UseBasicParsing "$repo/SHA256SUMS" -OutFile (Join-Path $tmp "SHA256SUMS")
  $line = Get-Content (Join-Path $tmp "SHA256SUMS") | Where-Object { $_ -match "\s+$([regex]::Escape($asset))$" } | Select-Object -First 1
  if (-not $line) { throw "No checksum published for $asset" }
  $expected = ($line -split "\s+")[0].ToLowerInvariant()
  $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($expected -ne $actual) { throw "SHA-256 mismatch for $asset" }
  Expand-Archive (Join-Path $tmp $asset) $tmp -Force
  $skillDir = Split-Path -Parent $PSScriptRoot
  $installDir = if ($env:LEXMOUNT_BROWSER_CLI_INSTALL_DIR) { $env:LEXMOUNT_BROWSER_CLI_INSTALL_DIR } else { Join-Path $skillDir "bin" }
  New-Item -ItemType Directory -Path $installDir -Force | Out-Null
  Copy-Item (Join-Path $tmp "browser-cli.exe") (Join-Path $installDir "browser-cli.exe") -Force
  & (Join-Path $installDir "browser-cli.exe") version
  Write-Output "Installed browser-cli to $installDir\browser-cli.exe"
} finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
