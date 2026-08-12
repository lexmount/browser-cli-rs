$ErrorActionPreference = "Stop"
$version = if ($env:LEXMOUNT_BROWSER_CLI_VERSION) { $env:LEXMOUNT_BROWSER_CLI_VERSION } else { "1.1.8" }
$architecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($architecture -ne "AMD64") { throw "Only Windows x64 is supported" }
$asset = "browser-cli-v$version-x86_64-pc-windows-msvc.exe"
$repo = "https://github.com/lexmount/browser-cli-rs/releases/download/v$version"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Invoke-WebRequest -UseBasicParsing "$repo/$asset" -OutFile (Join-Path $tmp $asset)
  Invoke-WebRequest -UseBasicParsing "$repo/SHA256SUMS" -OutFile (Join-Path $tmp "SHA256SUMS")
  # GNU sha256sum prefixes binary filenames with `*`; shasum uses plain whitespace.
  $line = Get-Content (Join-Path $tmp "SHA256SUMS") | Where-Object { $_ -match "\s+\*?$([regex]::Escape($asset))$" } | Select-Object -First 1
  if (-not $line) { throw "No checksum published for $asset" }
  $expected = ($line -split "\s+")[0].ToLowerInvariant()
  $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($expected -ne $actual) { throw "SHA-256 mismatch for $asset" }
  $skillDir = Split-Path -Parent $PSScriptRoot
  $installDir = if ($env:LEXMOUNT_BROWSER_CLI_INSTALL_DIR) { $env:LEXMOUNT_BROWSER_CLI_INSTALL_DIR } else { Join-Path $skillDir "bin" }
  New-Item -ItemType Directory -Path $installDir -Force | Out-Null
  Copy-Item (Join-Path $tmp $asset) (Join-Path $installDir "browser-cli.exe") -Force
  & (Join-Path $installDir "browser-cli.exe") version
  Write-Output "Installed browser-cli to $installDir\browser-cli.exe"
} finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }
