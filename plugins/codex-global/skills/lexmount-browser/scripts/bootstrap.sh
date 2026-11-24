#!/bin/sh
set -eu
version='1.5.0'
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target='x86_64-unknown-linux-musl'; expected='1b2ec45ff289a05d69a1a44e977c59f7f3a46b31d841184efb21dcb30f304f1f' ;;
  Darwin-arm64) target='aarch64-apple-darwin'; expected='bc78a1871caf0e00b75277b588c37e66ae709600a8c31dc182d8a4262b7c7e87' ;;
  *) echo 'This cloud edition supports Linux x86_64 and macOS arm64.' >&2; exit 2 ;;
esac
command -v python3 >/dev/null || { echo 'Python 3 is required for resumable device authorization.' >&2; exit 2; }
skill_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
asset="browser-cli-v$version-$target"
url="https://cli-bin-1377899528.cos.ap-nanjing.myqcloud.com/releases/browser-cli/v$version/$asset"
curl --proto '=https' --tlsv1.2 -fsSL --max-time 180 "$url" -o "$temporary/browser-cli"
if command -v sha256sum >/dev/null; then
  actual="$(sha256sum "$temporary/browser-cli" | awk '{print $1}')"
else
  actual="$(openssl dgst -sha256 "$temporary/browser-cli" | awk '{print $NF}')"
fi
[ "$actual" = "$expected" ] || { echo 'CLI SHA-256 verification failed.' >&2; exit 3; }
mkdir -p "$skill_dir/bin"
install -m 0755 "$temporary/browser-cli" "$skill_dir/bin/browser-cli"
"$skill_dir/bin/browser-cli" --version
