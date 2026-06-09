# DualBeam — Verbesserungsvorschläge

## Navigation & Ansicht
- Breadcrumb-Pfadleiste (klickbare Segmente)
- Type-ahead-Suche (Tippen springt zum passenden Eintrag)
- Quick-Look via Leertaste (`qlmanage -p`)
- Spaltenbreite/Sortierung pro Tab speichern
- Statuszeile: Anzahl/Größe Auswahl, freier Speicher des Volumes
- Symbole für iCloud-/Online-Only-Dateien, Aliase, versteckte Dateien

## Dateioperationen
- Drag & Drop zwischen den Panes (intern)
- Fortschrittsdialog mit Pause/Abbrechen für Kopier-/Verschiebejobs
- Kollisionsdialog: Überschreiben / Umbenennen / Überspringen / „für alle anwenden"
- Archive: ZIP erstellen aus Auswahl, Archive entpacken (zip/tar/gz)
- Ordnergröße auf Anfrage berechnen (Hotkey, z.B. Cmd+I)
- Rechte ändern (chmod-Dialog), Owner anzeigen
- Symlink/Alias gezielt per Menü anlegen

## Komfort
- Lesezeichen/Favoriten-Sidebar (inkl. Volumes, Home, iCloud, Schreibtisch)
- Papierkorb-Ansicht + Wiederherstellen
- „Terminal hier öffnen" / „Mit Editor öffnen" (Standardeditor wählbar)
- Synchronisierter Pfad-Modus (beide Panes folgen einander)
- Vergleichsmodus: gleiche/unterschiedliche Dateien zwischen Panes hervorheben

## System-Integration
- Theme automatisch dem System folgen
- Recent Folders im macOS-Menü
- Globaler Shortcut zum Fokussieren der App
- Dock-Badge bei laufenden Jobs

## Netzwerk / Remote
- ✅ **Verzeichnis-Synchronisation aktiver Pane → anderer Pane** (mit Vorschau, Update-/Lösch-Erkennung; funktioniert mit einem im Finder gemounteten HiDrive-Volume)
- WebDAV-Unterstützung als zweiter Pane-Backend-Typ neben lokalem FS (links lokal, rechts Remote)
  - Ziel-Storage: **IONOS HiDrive** (WebDAV-Endpoint `https://webdav.hidrive.ionos.com/`, Login mit HiDrive-Benutzername + Passwort/App-Passwort, HTTPS Pflicht)
  - Rust-Seite: Crate `reqwest_dav` oder `hyper` mit WebDAV-Methoden (`PROPFIND`, `MKCOL`, `MOVE`, `COPY`, `PUT`, `GET`, `DELETE`, `LOCK`)
  - TS-Alternative: npm `webdav` (CORS-Problem, daher besser via Tauri-Command)
  - Auth: HiDrive-App-Passwort statt Hauptpasswort verwenden; Credentials im macOS-Keychain ablegen
  - Verbindungs-Profile speichern (Name, URL, Benutzer, Root-Pfad)
  - Optional später: eingebauter WebDAV-Server (Rust `dav-server`) für P2P-Austausch im LAN — vom macOS Finder direkt mountbar
  - Stolpersteine bei HiDrive: viele kleine Dateien = viele HTTPS-Roundtrips (langsam); Range-Requests für große Dateien nutzen; ggf. Rate-Limits / Quota beachten; Locking nur bei Multi-User nötig
  - Referenz: manuelle Einrichtung in macOS (zur Validierung / als UX-Vorbild)
    1. Finder → Menü **Gehe zu** → **Mit Server verbinden…** (Cmd+K)
    2. Serveradresse: `https://webdav.hidrive.ionos.com/`
    3. Verbinden als: **Registrierter Benutzer**
    4. HiDrive-Benutzername + Passwort eingeben
    5. **Passwort im Schlüsselbund sichern** aktivieren
    6. **Verbinden** → Finder öffnet das Netzlaufwerk

## Sonstiges
- Tastatur-Cheatsheet (Cmd+/)
- Weitere Sprachen (FR/ES) — i18n-Gerüst steht
- Automatische Updates (Tauri Updater) + signierte/notarisierte DMG
  - ✅ In-App-Update-Prüfung + Direkt-Download der `.dmg` (via GitHub-Release, `download_and_open_update`); öffnet die DMG zum manuellen Ziehen in „Programme"
  - **Nach Signierung/Notarisierung:** Update-Flow auf vollautomatische Installation umstellen — entweder echter Tauri-Updater (signierte `.app.tar.gz` + Update-Signatur, Selbst-Ersetzung ohne Drag-and-Drop) oder still mounten + `ditto`/`rsync` der `.app` nach `/Applications` + Neustart; setzt gültige Developer-ID + Notarisierung voraus, damit Gatekeeper die ersetzte App akzeptiert
- Quellcode-Download-Button (MIT-Lizenz): im About-Dialog Link/Button „Quellcode herunterladen" → GitHub-Repo bzw. Source-Archiv des aktuellen Release (`https://github.com/nojan01/macos-dualpane/archive/refs/tags/<tag>.zip` oder Release-Seite)
- Signierter Privileged Helper (SMAppService) für Time-Machine-Löschvorgänge: ersetzt den Terminal.app-Umweg, vermeidet das kurze Aufblitzen eines Terminal-Fensters, behält FDA-Vererbung und macht das Passwort-Handling über XPC sauberer (kein temp-File auf Disk).
