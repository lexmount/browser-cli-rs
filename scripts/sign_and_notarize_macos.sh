#!/usr/bin/env bash
set -euo pipefail

binary="${1:?Usage: sign_and_notarize_macos.sh /path/to/binary}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${MACOS_DEVELOPER_ID_APPLICATION_P12_BASE64:?Missing MACOS_DEVELOPER_ID_APPLICATION_P12_BASE64}"
: "${MACOS_DEVELOPER_ID_P12_PASSWORD:?Missing MACOS_DEVELOPER_ID_P12_PASSWORD}"
: "${APPLE_NOTARY_APPLE_ID:?Missing APPLE_NOTARY_APPLE_ID}"
: "${APPLE_NOTARY_TEAM_ID:?Missing APPLE_NOTARY_TEAM_ID}"
: "${APPLE_NOTARY_APP_PASSWORD:?Missing APPLE_NOTARY_APP_PASSWORD}"

[[ -f "${binary}" ]] || { echo "Binary not found: ${binary}" >&2; exit 2; }

keychain_path="${RUNNER_TEMP}/lexmount-cli-signing.keychain-db"
certificate_path="${RUNNER_TEMP}/developer_id_application.p12"
notary_archive="${RUNNER_TEMP}/$(basename "${binary}").notary.zip"
notary_response="${RUNNER_TEMP}/$(basename "${binary}").notarization.json"
notary_profile="lexmount-cli-release-notary"
apple_pki_dir="${RUNNER_TEMP}/lexmount-cli-apple-pki"
keychain_password="$(openssl rand -hex 24)"

keychain_list="$(security list-keychains -d user 2>/dev/null || true)"
default_keychain="$(security default-keychain -d user 2>/dev/null | sed -E 's/^[[:space:]]*"(.*)"[[:space:]]*$/\1/' || true)"
existing_keychains=()
while IFS= read -r keychain; do
  keychain="$(printf '%s' "${keychain}" | sed -E 's/^[[:space:]]*"(.*)"[[:space:]]*$/\1/; s/^[[:space:]]+//; s/[[:space:]]+$//')"
  [[ -n "${keychain}" && -e "${keychain}" ]] && existing_keychains+=("${keychain}")
done <<< "${keychain_list}"

cleanup() {
  if [[ -n "${default_keychain}" && -e "${default_keychain}" ]]; then
    security default-keychain -d user -s "${default_keychain}" >/dev/null 2>&1 || true
  fi
  if ((${#existing_keychains[@]} > 0)); then
    security list-keychains -d user -s "${existing_keychains[@]}" >/dev/null 2>&1 || true
  fi
  security delete-keychain "${keychain_path}" >/dev/null 2>&1 || true
  rm -f "${certificate_path}" "${notary_archive}" "${notary_response}"
  rm -rf "${apple_pki_dir}"
}
trap cleanup EXIT INT TERM

printf '%s' "${MACOS_DEVELOPER_ID_APPLICATION_P12_BASE64}" | base64 -D >"${certificate_path}"
security create-keychain -p "${keychain_password}" "${keychain_path}"
security set-keychain-settings -lut 21600 "${keychain_path}"
security unlock-keychain -p "${keychain_password}" "${keychain_path}"
security list-keychains -d user -s "${keychain_path}" "${existing_keychains[@]}"
security default-keychain -d user -s "${keychain_path}"

mkdir -p "${apple_pki_dir}"
for certificate_spec in \
  'AppleWWDRCAG3.cer|DCF21878C77F4198E4B4614F03D696D89C66C66008D4244E1B99161AAC91601F' \
  'DeveloperIDCA.cer|7AFC9D01A62F03A2DE9637936D4AFE68090D2DE18D03F29C88CFB0B1BA63587F' \
  'DeveloperIDG2CA.cer|F16CD3C54C7F83CEA4BF1A3E6A0819C8AAA8E4A1528FD144715F350643D2DF3A'; do
  IFS='|' read -r certificate_name expected_fingerprint <<<"${certificate_spec}"
  apple_certificate="${apple_pki_dir}/${certificate_name}"
  curl --fail --silent --show-error --location --retry 3 --retry-delay 1 \
    --output "${apple_certificate}" \
    "https://www.apple.com/certificateauthority/${certificate_name}"
  actual_fingerprint="$(
    openssl x509 -inform der -in "${apple_certificate}" -noout -fingerprint -sha256 |
      awk -F= '{print $2}' | tr -d ':' | tr '[:lower:]' '[:upper:]'
  )"
  [[ "${actual_fingerprint}" == "${expected_fingerprint}" ]] || {
    echo "Unexpected Apple PKI certificate fingerprint: ${certificate_name}" >&2
    exit 3
  }
  security import "${apple_certificate}" -k "${keychain_path}" -T /usr/bin/security >/dev/null
done

security import "${certificate_path}" \
  -k "${keychain_path}" \
  -P "${MACOS_DEVELOPER_ID_P12_PASSWORD}" \
  -A \
  -T /usr/bin/codesign \
  -T /usr/bin/security
security set-key-partition-list \
  -S apple-tool:,apple:,codesign: \
  -s \
  -k "${keychain_password}" \
  "${keychain_path}"

identity="$(security find-identity -v -p codesigning "${keychain_path}" | awk -F'"' '/"Developer ID Application:/ {print $2; exit}')"
[[ -n "${identity}" ]] || { echo "Developer ID Application identity was not found" >&2; exit 4; }

codesign --force --options runtime --timestamp --sign "${identity}" "${binary}"
codesign --verify --strict --verbose=4 "${binary}"
codesign -dv --verbose=4 "${binary}" 2>&1 | grep -F "Authority=Developer ID Application:"

xcrun notarytool store-credentials "${notary_profile}" \
  --apple-id "${APPLE_NOTARY_APPLE_ID}" \
  --team-id "${APPLE_NOTARY_TEAM_ID}" \
  --password "${APPLE_NOTARY_APP_PASSWORD}" \
  --keychain "${keychain_path}"
ditto -c -k --keepParent "${binary}" "${notary_archive}"
xcrun notarytool submit "${notary_archive}" \
  --keychain-profile "${notary_profile}" \
  --keychain "${keychain_path}" \
  --wait \
  --output-format json >"${notary_response}"
cat "${notary_response}"
notary_status="$(plutil -extract status raw -o - "${notary_response}")"
[[ "${notary_status}" == "Accepted" ]] || {
  echo "Apple notarization status was ${notary_status}" >&2
  exit 5
}

echo "Signed and notarized ${binary} with ${identity}"
