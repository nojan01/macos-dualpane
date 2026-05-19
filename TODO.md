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

## Robustheit
- Watcher beider Panes beim Auswerfen eines Volumes proaktiv aufräumen
- Virtualisierte Liste (Windowing) für sehr große Verzeichnisse (>10k Einträge)
- Zugriffsfehler (Sandbox, Full-Disk-Access) freundlicher melden + Link in die Systemeinstellungen

## Sonstiges
- Tastatur-Cheatsheet (Cmd+/)
- Weitere Sprachen (FR/ES) — i18n-Gerüst steht
- Automatische Updates (Tauri Updater) + signierte/notarisierte DMG
