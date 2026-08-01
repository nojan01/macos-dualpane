#!/bin/bash
# Legt die Lizenztexte an die Stelle, von der Tauri sie ins Programmpaket
# uebernimmt (src-tauri/resources/licenses -> DualBeam.app/Contents/Resources).
#
# Warum ueberhaupt kopieren: EULA.md und THIRD_PARTY_LICENSES.md gehoeren als
# Quelle in die Wurzel des Projekts, damit GitHub sie anzeigt. Im Programmpaket
# muessen sie aber ebenfalls liegen - die MIT-Lizenz von rclone und die
# Apache-Lizenz der Tauri-Bausteine verlangen, dass Urhebervermerk und
# Lizenztext der Weitergabe beiliegen. Zwei gepflegte Kopien waeren eine
# Fehlerquelle, deshalb wird bei jedem Bau kopiert statt eingecheckt.
#
# RCLONE-LICENSE.txt und APACHE-2.0.txt kommen nicht von hier: die erste holt
# scripts/fetch-rclone.sh am passenden Versionsschild, die zweite liegt fest.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="$repo_root/src-tauri/resources/licenses"
mkdir -p "$target"

cp "$repo_root/EULA.md" "$target/EULA.txt"
cp "$repo_root/THIRD_PARTY_LICENSES.md" "$target/THIRD-PARTY-LICENSES.txt"

# Fehlt einer der beiden fest hinterlegten Texte, bricht der Bau lieber ab, als
# ein unvollstaendiges Paket zu erzeugen.
for required in RCLONE-LICENSE.txt APACHE-2.0.txt MIT.txt; do
  if [ ! -s "$target/$required" ]; then
    echo "Lizenztext fehlt: $required" >&2
    echo "  RCLONE-LICENSE.txt legt scripts/fetch-rclone.sh an." >&2
    exit 1
  fi
done

echo "Lizenztexte bereit: $(ls "$target" | wc -l | tr -d ' ') Dateien in ${target#"$repo_root"/}"
