# DualBeam – Code-Signierung & Notarisierung (macOS)

Anleitung, um DualBeam für macOS digital zu signieren und zu notarisieren, damit
die App ohne Gatekeeper-Warnung („nicht verifizierter Entwickler") auf anderen
Macs läuft.

## Aktueller Stand

Signierung und Notarisierung sind eingerichtet und laufen ueber ein Skript:

```bash
scripts/release-macos.sh              # persoenliche Variante (mit HiDrive)
scripts/release-macos.sh --public     # oeffentliche Variante (ohne HiDrive)
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

## RemoteDeskRDP (remote-client/)

Der RDP-Client wird getrennt gebaut und notarisiert:

```bash
cd remote-client
npm run tauri:build    # baut, signiert dabei auch das FreeRDP-Backend
npm run notarize       # siegelt nach, reicht ein, heftet das Ticket an
```

Zugangsdaten liegen als notarytool-Profil `remotedesk-notary` im Schluesselbund:

```bash
xcrun notarytool store-credentials "remotedesk-notary" --team-id TXF2V79Z6N
```

### Zwei Fallen, die je einen Fehlversuch kosten

**1. Tauri liefert `resources/` unsigniert durch.** Das mitgelieferte
FreeRDP-Backend besteht aus 35 Mach-O-Dateien. Tauri signiert nur die aeussere
App und die eigenen Binaerdateien; alles unter `resources/` bleibt unberuehrt.
Ohne Signatur lehnt Apple ab. `scripts/sign-freerdp-backend.sh` erledigt das und
haengt fest im Bauablauf (`npm run tauri:build`).

**2. Tauri loest beim Kopieren Symlinks auf.** Das Backend enthaelt 34 davon
(`libz.1.dylib` → `libz.1.4.1.1.dylib` und aehnliche). Aus 44 Dateien mit 47 MB
werden dabei 78 Dateien mit 111 MB. `MacFreeRDP.app` ist aber ein **eigenes
Bundle mit eigenem Siegel**, und dieses Siegel haelt die Symlinks fest. Nach dem
Kopieren passt es nicht mehr:

```
codesign --verify ...  ->  file modified: .../libz.1.dylib
```

Das wird leicht uebersehen, weil die **aeussere App trotzdem sauber
verifiziert** – auch mit `--deep --strict`; ihr Siegel erfasst `Resources/` nur
als Daten. Apples Notardienst prueft genauer:

```
The signature of the binary is invalid
.../MacFreeRDP.app/Contents/MacOS/MacFreeRDP
```

Belegt am 01.08.2026: Vorgang `4f1fd1ef-…` abgelehnt (zwei Beanstandungen, beide
an `MacFreeRDP`), derselbe Bau nach dem Nachsiegeln als `bb133bd5-…` angenommen.

Deshalb siegelt `scripts/notarize.sh` das eingebettete Bundle nach dem Bauen neu
und signiert anschliessend die aeussere App – **ohne `--deep`**. Mit `--deep`
wuerde codesign die eingebetteten Bundles mit der Kennung der aeusseren App
ueberschreiben und das eben erneuerte Siegel wieder zerstoeren.

> Bewusst zurueckgestellt: Die aufgeloesten Symlinks kosten rund 64 MB.
> Zusammenlegen ginge nur ueber `install_name_tool` an 35 Binaerdateien, weil
> tatsaechlich **beide** Namensvarianten geladen werden (`libz.1.dylib` 18×,
> `libz.1.4.1.1.dylib` 1×). Der Aufwand steht derzeit nicht dafuer.

### Werkzeugtuecken beim Signieren

- **Von innen nach aussen signieren.** Wird die Huelle zuerst signiert,
  entwertet jede spaetere Signatur im Inneren das aeussere Siegel.
- **Die Hauptdatei eines Bundles nie einzeln signieren.** codesign erkennt
  `Contents/MacOS/<CFBundleExecutable>` als Bundle-Hauptdatei und signiert
  daraufhin das *gesamte* Bundle. Mitten in einer Schleife sind die uebrigen
  Dateien dann noch unsigniert und der Lauf bricht ab mit
  „code object is not signed at all / In subcomponent: …".
- **Symlinks tragen keine Signatur.** `find -type f` laesst sie aus.
- **Fette Binaerdateien** melden bei `file` eine Zeile *je Architektur*
  (`… (for architecture arm64):`). Ohne Nachbearbeitung entstehen daraus
  ungueltige Pfade – im ersten Anlauf 105 statt 35 Dateien.

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

## Build-Varianten: persönlich vs. öffentlich

Es gibt zwei Build-Varianten, gesteuert über das Cargo-Feature `hidrive`:

| Variante | Befehl | IONOS-HiDrive-Voreinstellung |
| --- | --- | --- |
| **Persönlich** (Standard) | `npm run tauri:build` | enthalten |
| **Öffentlich** (Release) | `npm run tauri:build:public` | entfernt |

- Die öffentliche Version wird mit `tauri build --no-default-features` gebaut.
  Dadurch wird der HiDrive-Code per `#[cfg(feature = "hidrive")]` gar nicht erst
  einkompiliert – es landet **keine** personenbezogene Voreinstellung im Binary.
- Die generische Netzwerk-Funktion (beliebige WebDAV/SMB-URL verbinden, mounten,
  trennen) bleibt in beiden Varianten erhalten; nur das fest vorkonfigurierte
  HiDrive-Lesezeichen entfällt in der öffentlichen Version.
- Für die Veröffentlichung **immer** `npm run tauri:build:public` verwenden.

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
