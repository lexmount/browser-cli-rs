#!/usr/bin/env bash
set -euo pipefail

source_dir="${1:?Usage: upload-release-to-cos.sh SOURCE_DIR PRODUCT VERSION}"
product="${2:?Usage: upload-release-to-cos.sh SOURCE_DIR PRODUCT VERSION}"
version="${3:?Usage: upload-release-to-cos.sh SOURCE_DIR PRODUCT VERSION}"

: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${TENCENT_CLOUD_SECRET_ID:?Missing TENCENT_CLOUD_SECRET_ID}"
: "${TENCENT_CLOUD_SECRET_KEY:?Missing TENCENT_CLOUD_SECRET_KEY}"
: "${COS_BUCKET:?Missing COS_BUCKET}"
: "${COS_REGION:?Missing COS_REGION}"
: "${COS_PUBLIC_BASE_URL:?Missing COS_PUBLIC_BASE_URL}"
: "${COS_OBJECT_PREFIX:?Missing COS_OBJECT_PREFIX}"

[[ -d "${source_dir}" ]] || { echo "Source directory not found: ${source_dir}" >&2; exit 2; }
[[ -f "${source_dir}/SHA256SUMS" ]] || { echo "Checksum manifest not found: ${source_dir}/SHA256SUMS" >&2; exit 2; }
[[ "${product}" =~ ^[a-z0-9-]+$ ]] || { echo "Invalid product path: ${product}" >&2; exit 2; }
[[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Invalid version: ${version}" >&2; exit 2; }

coscli_version="1.0.8"
coscli_name="coscli-v${coscli_version}-linux-amd64"
coscli_path="${RUNNER_TEMP}/${coscli_name}"
coscli_sha256="7165f2ae16c5f7ac495864c963ca574a76e04ec72680d7bc8a8eee3234d8cf91"
verify_dir="${RUNNER_TEMP}/${product}-cos-verify"
object_prefix="${COS_OBJECT_PREFIX#/}"
object_prefix="${object_prefix%/}"
remote_path="${object_prefix}/${product}/v${version}"
public_url="${COS_PUBLIC_BASE_URL%/}/${remote_path}"

curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 \
  "https://github.com/tencentyun/coscli/releases/download/v${coscli_version}/${coscli_name}" \
  --output "${coscli_path}"
printf '%s  %s\n' "${coscli_sha256}" "${coscli_path}" | sha256sum --check --strict
chmod 0700 "${coscli_path}"

"${coscli_path}" cp "${source_dir}/" "cos://${COS_BUCKET}/${remote_path}/" \
  --recursive \
  --endpoint "cos.${COS_REGION}.myqcloud.com" \
  --secret-id "${TENCENT_CLOUD_SECRET_ID}" \
  --secret-key "${TENCENT_CLOUD_SECRET_KEY}" \
  --init-skip=true \
  --disable-log

mkdir -p "${verify_dir}"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 5 \
  "${public_url}/SHA256SUMS" --output "${verify_dir}/SHA256SUMS"
cmp "${source_dir}/SHA256SUMS" "${verify_dir}/SHA256SUMS"

while read -r expected file_name; do
  file_name="${file_name#\*}"
  [[ "${file_name}" != */* && -n "${file_name}" ]] || { echo "Invalid manifest filename: ${file_name}" >&2; exit 3; }
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --retry 5 \
    "${public_url}/${file_name}" --output "${verify_dir}/${file_name}"
  printf '%s  %s\n' "${expected}" "${verify_dir}/${file_name}" | sha256sum --check --strict
done <"${source_dir}/SHA256SUMS"

echo "Uploaded and publicly verified ${product} v${version} at ${public_url}"
