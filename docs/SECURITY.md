# Abhängigkeiten und Sicherheitsprüfungen

Stand: 22. November 2026. Die Ergebnisse sind Momentaufnahmen bekannter
Schwachstellen, kein Nachweis vollständiger Sicherheit oder Erreichbarkeit.

## Reproduzierbare Prüfung

```sh
npm ci
npm audit --audit-level=low
cargo audit --file src-tauri/Cargo.lock
bash scripts/fetch-rclone.sh
python3 scripts/audit-rclone.py src-tauri/binaries/rclone-aarch64-apple-darwin
```

Auf Intel-Macs beim letzten Befehl `rclone-x86_64-apple-darwin` verwenden.
CI verwendet npm 11.21.3; ältere npm-10-Versionen können beim Auflösen der
aktuellen Peer-Abhängigkeiten intern fehlschlagen. Lockdateien einchecken und
nicht durch unkontrollierte Neuauflösung ersetzen.

Nach der Aktualisierung: keine Schwachstellentreffer in npm, Cargo oder dem
versionsbasierten OSV-Abgleich der 238 rclone-/Go-Module. Cargo meldet weiterhin
Warnungen zu `chrono`, `proc-macro-error` und `glib`; diese Pakete gehören nicht
zum macOS-arm64-Abhängigkeitsbaum. Sie werden nicht per Ignore-Liste ausgeblendet.
Vor Unterstützung anderer Plattformen müssen diese Warnungen erneut bewertet werden.

## rclone-Sicherheitsbuild

`scripts/rclone/build.json` bindet Quellversion, Go-Werkzeugkette, Lizenz und
beide Binärdateien an Prüfsummen. `go.mod` und `go.sum` fixieren die aktualisierten
Go-Abhängigkeiten. `scripts/fetch-rclone.sh all` baut fehlende oder veränderte
Sidecars aus verifizierten Quellen und prüft das Ergebnis vor der Verwendung.
Ein Cache-Treffer wird ebenfalls vollständig gehasht, niemals zur Versionsprüfung
ausgeführt. Der erste Build benötigt Internetzugriff und mehr Zeit als ein Download.

Der Build nutzt Go 1.27.4, rclone 1.76.0, x/crypto 0.59.0 und OpenTelemetry 1.47.0.
CGO ist deaktiviert; das von DualBeam verwendete `nfsmount` ist enthalten,
das nicht verwendete `cmount` nicht. Der OSV-Prüfer akzeptiert `(devel)` als
Hauptmodulversion nur bei einer zum Manifest passenden Binärprüfsumme und
prüft dann die verifizierte Quellversion. Netzwerk- und Parsefehler führen zum
Fehlschlag, nicht zu einem vermeintlich sauberen Ergebnis.

## Externe Werkzeuge

SSHFS vor 3.7.6 wird für SFTP-Mounts abgelehnt (unter anderem CVE-2026-47187).
Zusätzlich wird `contain_symlinks` angefordert. Eine kompatible, gepatchte
macFUSE-/SSHFS-Installation muss separat bereitgestellt werden; DualBeam
ersetzt keine systemweiten Dateisystemtreiber. Die lokal vorgefundene Version
3.7.5 bleibt installiert, wird von diesem Entwicklungsstand aber nicht verwendet.

macOS-eigene Werkzeuge und macFUSE sind nicht durch npm/Cargo/OSV vollständig
abgedeckt. Apple-Sicherheitsupdates bleiben notwendig. Nach dem Entfernen der
Hardened-Runtime-Ausnahmen für unsignierten ausführbaren Speicher und deaktivierte
Bibliotheksvalidierung ist vor Veröffentlichung ein signierter App-Smoke-Test nötig.

## SMB-Livetest

`remote::live_tests::saved_smb_profile_roundtrip` ist standardmäßig ignoriert.
Er benötigt explizit `DUALBEAM_TEST_PROFILE_DB` und Schlüsselbund-Freigabe,
verwendet ein gespeichertes SMB-Profil, aber kein Sync-Profil, und arbeitet nur
in einem neu angelegten eindeutigen Testverzeichnis. Kein Lauf ohne Zustimmung
zum Schreiben von Testdaten auf der Freigabe.

Prüfstand dieser Änderung: 143 Rust-Tests, 81 Frontend-Tests und vier
Scanner-Regressionstests erfolgreich; Rust-Clippy mit `-D warnings` und
Frontend-Produktionsbuild erfolgreich. Beide rclone-Architekturen wurden aus
einem zweiten Arbeitsverzeichnis mit identischen Prüfsummen nachgebaut.

Auf der vorhandenen SMB-Verbindung waren Lesen, Schreiben, SHA256-Vergleich,
Umbenennen, Überschreiben und Löschen eigener Testdaten erfolgreich. Der erste
separate Mount-Test zeigte einen veralteten Mount-Cache bei direktem Download;
Dateitypprüfung und Löschklassifikation wurden auf direkte Serverabfragen
umgestellt. Der Wiederholungstest wurde vor dem Mount beendet, weil die
Schlüsselbund-Freigabe ausstand. Dieser Live-Nachtest ist noch offen.

## Freigabe-Kurztest für 0.5.19

Am 22. November 2026 wurden Swift, S3, SMB und NFS in der signierten
Test-App 0.5.19-test.3 einzeln neu verbunden und ihre Wurzelverzeichnisse
erfolgreich gelesen. Keine bestehenden Nutzdaten wurden dabei verändert;
dieser Kurztest ist kein vollständiger Schreib-/Sync-Test.

Eine Blockade durch einen nicht erreichbaren SMB-Mount wurde behoben:
Mount-Erkennung verwendet die gecachte Kernel-Tabelle, direkte Remote-Pfade
werden ohne Zugriff auf den Mount aufgelöst, und die gemeinsame Registrierung
wird bei der Objekt-Speicher- und rclone-Pfadzuordnung vor Dateisystemzugriffen
wieder freigegeben.
„No route to host“ wird als Erreichbarkeitsfehler behandelt und führt nicht mehr
zu einem weiteren Mount-Versuch. Die Korrektur wurde zusätzlich lesend am
bereits blockierten Mount geprüft. Nach dem Zurücksetzen der lokalen
macOS-Netzwerkfreigabe und Neustart funktionierte auch SMB wieder in der App.

Vor dem lokalen 0.5.19-Build erneut erfolgreich: 81 Frontend-Tests und
144 Rust-Tests. Acht opt-in Live-/Umgebungstests bleiben standardmäßig ignoriert.
Der lokale Build allein ist keine Notarisierung oder GitHub-Veröffentlichung.
