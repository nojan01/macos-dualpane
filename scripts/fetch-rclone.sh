#!/bin/bash
# Reproducible security build; never execute an unverified cached sidecar.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
manifest="$repo_root/scripts/rclone/build.json"
read_config() { node -e 'let c=require(process.argv[1]);for(const k of process.argv[2].split("."))c=c[k];if(typeof c!=="string")process.exit(1);process.stdout.write(c)' "$manifest" "$1"; }
version="$(read_config version)"
go_version="$(read_config goVersion)"
source_version="$(read_config sourceVersion)"
source_sum="$(read_config sourceSum)"
case "$(uname -m)" in arm64) native=arm64 ;; x86_64) native=amd64 ;; *) echo 'Unsupported host architecture' >&2; exit 1 ;; esac
case "${1:-native}" in native) arches="$native" ;; all) arches='arm64 amd64' ;; arm64|amd64) arches="$1" ;; *) echo 'Expected native, all, arm64 or amd64' >&2; exit 1 ;; esac
target="$repo_root/src-tauri/binaries"
cache="$repo_root/src-tauri/target/rclone-security-cache"
mkdir -p "$target" "$cache"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
initialized=false
for arch in $arches; do
  case "$arch" in arm64) triple=aarch64-apple-darwin ;; amd64) triple=x86_64-apple-darwin ;; esac
  dest="$target/rclone-$triple"
  expected="$(read_config binarySha256.$arch)"
  if [ -f "$dest" ] && [ "$(shasum -a 256 "$dest" | awk '{print $1}')" = "$expected" ]; then
    echo "rclone $version ($arch): SHA256 verified."
  else
    if ! $initialized; then
      archive="$cache/$go_version.darwin-$native.tar.gz"
      tool_hash="$(read_config goArchiveSha256.$native)"
      if [ ! -f "$archive" ] || [ "$(shasum -a 256 "$archive" | awk '{print $1}')" != "$tool_hash" ]; then
        curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
          "https://go.dev/dl/$go_version.darwin-$native.tar.gz" -o "$work/go.tar.gz"
        [ "$(shasum -a 256 "$work/go.tar.gz" | awk '{print $1}')" = "$tool_hash" ] || { echo 'Go toolchain checksum mismatch' >&2; exit 1; }
        mv "$work/go.tar.gz" "$archive"
      fi
      tar -xzf "$archive" -C "$work"
      export PATH="$work/go/bin:$PATH" GOTOOLCHAIN=local GOENV=off GOWORK=off GOFLAGS='' GOEXPERIMENT=''
      export GOPATH="$cache/gopath" GOCACHE="$cache/gocache" GOSUMDB=sum.golang.org GOPROXY=https://proxy.golang.org
      export GOPRIVATE='' GONOSUMDB='' GONOPROXY='' CGO_ENABLED=0 GOOS=darwin GOAMD64=v1 GOARM64=v8.0
      go mod download -json "github.com/rclone/rclone@$source_version" > "$work/source.json"
      source_dir="$(node -e 'const r=require(process.argv[1]);if(r.Error||r.Sum!==process.argv[2])process.exit(1);process.stdout.write(r.Dir)' "$work/source.json" "$source_sum")"
      cp -R "$source_dir" "$work/source"
      chmod -R u+w "$work/source"
      cp "$repo_root/scripts/rclone/go.mod" "$work/source/go.mod"
      cp "$repo_root/scripts/rclone/go.sum" "$work/source/go.sum"
      (cd "$work/source" && go mod download && go mod verify)
      initialized=true
    fi
    echo "Building rclone $version ($arch) with $go_version..."
    (cd "$work/source" && GOARCH="$arch" go build -mod=readonly -trimpath -buildvcs=false \
      -ldflags "-s -w -X github.com/rclone/rclone/fs.Version=$version" -o "$work/rclone-$arch" .)
    actual="$(shasum -a 256 "$work/rclone-$arch" | awk '{print $1}')"
    [ "$actual" = "$expected" ] || { echo "Reproducible build mismatch ($arch): $actual" >&2; exit 1; }
    mv "$work/rclone-$arch" "$dest"
    chmod +x "$dest"
  fi
done

# Also repair missing licenses on a binary cache hit.
licenses="$repo_root/src-tauri/resources/licenses"
mkdir -p "$licenses"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
  "https://raw.githubusercontent.com/rclone/rclone/$source_version/COPYING" -o "$work/COPYING"
[ "$(shasum -a 256 "$work/COPYING" | awk '{print $1}')" = "$(read_config licenseSha256)" ] || { echo 'rclone license checksum mismatch' >&2; exit 1; }
cp "$work/COPYING" "$licenses/RCLONE-LICENSE.txt"
