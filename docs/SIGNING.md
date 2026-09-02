# DualBeam – Code-Signierung & Notarisierung (macOS)

Anleitung, um DualBeam für macOS digital zu signieren und zu notarisieren, damit
die App ohne Gatekeeper-Warnung („nicht verifizierter Entwickler") auf anderen
Macs läuft.

## Aktueller Stand

Signierung und Notarisierung sind eingerichtet und laufen ueber ein Skript:

```bash
scripts/release-macos.sh              # bauen, signieren, notarisieren
scripts/release-macos.sh --reset      # hinterlegte Zugangsdaten loeschen
```

Das Skript holt Signaturkennung und Team-ID aus `src-tauri/tauri.conf.json`,
fragt die Zugangsdaten fuer die Notarisierung beim ersten Lauf einmalig ab und
hinterlegt sie im Schluesselbund. Danach laeuft es ohne Eingabe. Am Ende prueft
es Signatur, Ticket, das mitgelieferte `rclone` und das Gatekeeper-Urteil nach
und bricht ab, wenn eine dieser Pruefungen fehlschlaegt.

Die Abschnitte 1 bis 5 beschreiben, was dahinter steckt und was bei einem
Zertifikatswechsel oder auf einem neuen Rechner zu tun ist.

---

## 1. Voraussetzung: Apple Developer Account

- **Apple Developer Program** Mitgliedschaft (99 USD/Jahr): https://developer.apple.com
- Ohne Mitgliedschaft ist nur „ad-hoc"-Signierung möglich (lokal lauffähig, aber
  andere Nutzer bekommen weiterhin die Gatekeeper-Warnung).

## 2. Zertifikat erstellen & installieren

Benötigt wird ein **„Developer ID Application"**-Zertifikat (Verteilung außerhalb
des App Store):

1. In Xcode: *Settings → Accounts → Apple-ID hinzufügen → Manage Certificates →
   + → Developer ID Application*.
2. Alternativ im Developer-Portal: *Certificates → + → Developer ID Application*,
   CSR per Schlüsselbundverwaltung erzeugen, hochladen, herunterladen,
   doppelklicken.

Danach erscheint es bei:

```bash
security find-identity -v -p codesigning
```

z. B. als `Developer ID Application: Dein Name (TEAMID)`.

## 3. Tauri zum Signieren konfigurieren

Tauri signiert beim Build automatisch, wenn diese Umgebungsvariable gesetzt ist:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Dein Name (TEAMID)"
```

Optional fest in `src-tauri/tauri.conf.json` eintragen:

```jsonc
"macOS": {
  "minimumSystemVersion": "12.0",
  "signingIdentity": "Developer ID Application: Dein Name (TEAMID)"
}
```

## 4. Hardened Runtime + Entitlements (für Notarisierung Pflicht)

Notarisierung verlangt „Hardened Runtime". Weil DualBeam JIT/Cocoa/Frameworks
nutzt, eine Entitlements-Datei `src-tauri/entitlements.plist` anlegen:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.cs.allow-jit</key><true/>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
  <key>com.apple.security.cs.disable-library-validation</key><true/>
</dict>
</plist>
```

und in `src-tauri/tauri.conf.json` referenzieren:

```jsonc
"macOS": {
  "entitlements": "entitlements.plist"
}
```

## 5. Notarisieren (Apple-Stempel)

Tauri notarisiert automatisch, wenn zusätzlich gesetzt ist:

```bash
# Variante A: App-Specific Password
export APPLE_ID="deine-apple-id@example.com"
export APPLE_PASSWORD="app-spezifisches-passwort"   # appleid.apple.com → Anmeldung & Sicherheit
export APPLE_TEAM_ID="TEAMID"

# Variante B: API Key (.p8 von App Store Connect)
export APPLE_API_ISSUER="..."
export APPLE_API_KEY="..."
export APPLE_API_KEY_PATH="/Pfad/AuthKey_XXXX.p8"
```

Dann normal bauen:

```bash
npm run tauri:build
```

Tauri signiert → notarisiert → „stapelt" (staplet) das Ticket automatisch in
DMG/App.

> Genau das erledigt `scripts/release-macos.sh`. Es setzt Variante A, zieht das
> Passwort aber aus dem Schlüsselbund statt aus der Umgebung. Werden die
> Variablen nur im Terminal gesetzt, sind sie nach dessen Ende weg — bei 0.3.0
> war das der Fall, für 0.4.0 mussten sie deshalb neu beschafft werden.

**Wichtig: Tauri notarisiert nur die `.app`** und stapelt das Ticket auch nur
dort. Das DMG wird zwar signiert, bleibt aber ohne Ticket. Beim Öffnen einer
geladenen Datei prüft Gatekeeper jedoch zuerst das DMG. Es muss deshalb getrennt
eingereicht werden — `scripts/release-macos.sh` tut das im Anschluss an den
Build:

```bash
xcrun notarytool submit DualBeam_0.4.0_aarch64.dmg \
  --apple-id "…" --password "…" --team-id "TXF2V79Z6N" --wait
xcrun stapler staple DualBeam_0.4.0_aarch64.dmg
```

## 6. Verifizieren

```bash
codesign --verify --deep --strict --verbose=2 \
  src-tauri/target/release/bundle/macos/DualBeam.app

spctl -a -vvv -t install \
  src-tauri/target/release/bundle/macos/DualBeam.app

xcrun stapler validate \
  src-tauri/target/release/bundle/dmg/DualBeam_0.4.0_aarch64.dmg
```

---

## Kurzfassung der To-dos

1. ~~Apple Developer Program beitreten (99 USD/Jahr).~~ **erledigt**
2. ~~„Developer ID Application"-Zertifikat erstellen & installieren.~~ **erledigt**
3. ~~`APPLE_SIGNING_IDENTITY` setzen (oder in `tauri.conf.json`).~~ **erledigt**
4. ~~`entitlements.plist` mit Hardened-Runtime-Rechten anlegen und verlinken.~~ **erledigt**
5. ~~Notarisierungs-Credentials setzen.~~ **erledigt** (im Schlüsselbund, siehe oben)
6. `scripts/release-macos.sh` aufrufen → fertig signiert & notarisiert.

---

## Mitgeliefertes `rclone`

Für SFTP und FTPS liegt `rclone` als Beiprogramm im Paket unter
`Contents/MacOS/rclone`. Es wird nicht im Git verwaltet, sondern von
`scripts/fetch-rclone.sh` geladen und anhand seiner Prüfsumme geprüft; der
Build ruft das Skript automatisch auf.

Für die Notarisierung muss das Beiprogramm **mitsigniert** sein. Nach dem Build
prüfen:

```sh
codesign -dv --verbose=4 \
  "src-tauri/target/release/bundle/macos/DualBeam.app/Contents/MacOS/rclone"
```

Erwartet wird eine Signatur mit derselben Developer-ID und aktiviertem Hardened
Runtime wie bei der App selbst. Fehlt sie, weist Apple das Paket zurück.

> Hinweis: Sollen beide Varianten parallel auf demselben Mac installierbar sein,
> in `src-tauri/tauri.conf.json` für die öffentliche Version ggf. eigene
> `identifier`/`productName` vergeben (sonst überschreiben sie sich gegenseitig).
