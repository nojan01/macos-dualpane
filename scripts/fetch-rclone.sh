#!/bin/bash
# Laedt die mitgelieferte rclone-Binaerdatei herunter und legt sie unter dem
# Namen ab, den Tauri fuer Sidecars erwartet (<name>-<target-triple>).
#
# rclone stellt SFTP und FTPS als ganz normales Laufwerk bereit. Es haengt ueber
# den NFS-Client von macOS ein, deshalb wird weder eine Kernel-Erweiterung noch
# ein Administratorkennwort gebraucht.
#
# Die Datei ist bewusst nicht eingecheckt: rund 78 MB gehoeren nicht in die
# Versionsverwaltung. Stattdessen ist die Version hier festgenagelt und die
# Pruefsumme wird gegen die von rclone veroeffentlichten Werte geprueft.
set -euo pipefail

RCLONE_VERSION="v1.74.4"

# Aus den offiziellen SHA256SUMS von https://downloads.rclone.org/
SHA256_ARM64="c2100e2d4a4b3be04c55cd45380cafe7647e1ad772bb055f52f00876ed701167"
SHA256_AMD64="4188aa84043d7a6240912923f47639a9d2da21f3b40a521c065c8d92e66563f6"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_dir="$repo_root/src-tauri/binaries"
mkdir -p "$target_dir"

# Ohne Argument nur die Architektur des laufenden Rechners. Das haelt den
# normalen Build kurz; "all" ist fuer einen spaeteren Universal-Build da.
case "${1:-native}" in
  all) arches="arm64 amd64" ;;
  native) [ "$(uname -m)" = "x86_64" ] && arches="amd64" || arches="arm64" ;;
  arm64 | amd64) arches="$1" ;;
  *)
    echo "Unbekanntes Ziel: $1 (erlaubt: native, all, arm64, amd64)" >&2
    exit 1
    ;;
esac

for arch in $arches; do
  case "$arch" in
    arm64)
      triple="aarch64-apple-darwin"
      expected="$SHA256_ARM64"
      ;;
    amd64)
      triple="x86_64-apple-darwin"
      expected="$SHA256_AMD64"
      ;;
  esac

  dest="$target_dir/rclone-$triple"
  if [ -x "$dest" ]; then
    have="$("$dest" version 2>/dev/null | head -1 | awk '{print $2}')" || have=""
    if [ "$have" = "$RCLONE_VERSION" ]; then
      echo "rclone $RCLONE_VERSION fuer $arch liegt bereits vor."
      continue
    fi
  fi

  archive="rclone-$RCLONE_VERSION-osx-$arch.zip"
  url="https://downloads.rclone.org/$RCLONE_VERSION/$archive"
  work="$(mktemp -d)"
  # Auch bei Abbruch oder Fehler nichts liegen lassen.
  trap 'rm -rf "$work"' EXIT

  echo "Lade rclone $RCLONE_VERSION fuer $arch ..."
  curl --fail --silent --show-error --location --output "$work/$archive" "$url"

  actual="$(shasum -a 256 "$work/$archive" | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "Pruefsumme von $archive stimmt nicht." >&2
    echo "  erwartet: $expected" >&2
    echo "  erhalten: $actual" >&2
    exit 1
  fi

  unzip -q "$work/$archive" -d "$work"
  # Erst an die endgueltige Stelle schieben, wenn alles geprueft ist. Sonst
  # koennte ein abgebrochener Lauf eine halbe Datei hinterlassen, die der
  # naechste Build fuer fertig haelt.
  mv "$work/rclone-$RCLONE_VERSION-osx-$arch/rclone" "$dest"
  chmod +x "$dest"

  rm -rf "$work"
  trap - EXIT
  echo "Abgelegt: ${dest#"$repo_root"/}"
done
