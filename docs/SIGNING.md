# DualBeam – Code-Signierung & Notarisierung (macOS)

Anleitung, um DualBeam für macOS digital zu signieren und zu notarisieren, damit
die App ohne Gatekeeper-Warnung („nicht verifizierter Entwickler") auf anderen
Macs läuft.

## Aktueller Stand

- Code-Signing-Zertifikate im Schlüsselbund: **keine** (`security find-identity -v -p codesigning` → 0 valid identities).
- Es muss also zuerst ein Zertifikat beschafft werden.

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

## 6. Verifizieren

```bash
codesign --verify --deep --strict --verbose=2 \
  src-tauri/target/release/bundle/macos/DualBeam.app

spctl -a -vvv -t install \
  src-tauri/target/release/bundle/macos/DualBeam.app

xcrun stapler validate \
  src-tauri/target/release/bundle/dmg/DualBeam_0.2.0_aarch64.dmg
```

---

## Kurzfassung der To-dos

1. Apple Developer Program beitreten (99 USD/Jahr).
2. „Developer ID Application"-Zertifikat erstellen & installieren.
3. `APPLE_SIGNING_IDENTITY` setzen (oder in `tauri.conf.json`).
4. `entitlements.plist` mit Hardened-Runtime-Rechten anlegen und verlinken.
5. Notarisierungs-Credentials (`APPLE_ID` / `APPLE_PASSWORD` / `APPLE_TEAM_ID`) setzen.
6. `npm run tauri:build` → fertig signiert & notarisiert.

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

> Hinweis: Sollen beide Varianten parallel auf demselben Mac installierbar sein,
> in `src-tauri/tauri.conf.json` für die öffentliche Version ggf. eigene
> `identifier`/`productName` vergeben (sonst überschreiben sie sich gegenseitig).
