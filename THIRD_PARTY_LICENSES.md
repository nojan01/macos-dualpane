# Verwendete Fremdkomponenten und deren Lizenzen — DualBeam

Stand: 22. November 2026, Entwicklungsstand nach Sicherheitsaktualisierung

DualBeam selbst steht unter der [MIT-Lizenz](LICENSE). Für die hier aufgeführten
Fremdkomponenten gelten zusätzlich deren eigene Lizenzen.

---

## 1. Mitgeliefertes Programm

Genau ein fremdes Programm wird als Datei mitgeliefert:

| Komponente | Version | Lizenz | Urheber |
| --- | --- | --- | --- |
| [rclone](https://rclone.org) | 1.76.0-dualbeam.1 | MIT | Nick Craig-Wood und Mitwirkende |

rclone liegt im Programmpaket unter `DualBeam.app/Contents/MacOS/rclone` und
stellt unter anderem SMB-, FTPS- und Objekt-Speicher-Verbindungen bereit. Der vollständige
Lizenztext liegt bei (`RCLONE-LICENSE.txt`).

> rclone ist in Go geschrieben und enthält seinerseits zahlreiche
> Open-Source-Bibliotheken unter eigenen Lizenzen. Deren Auflistung ist Teil des
> rclone-Projekts. DualBeam erstellt rclone 1.76.0 mit Go 1.27.4 und aktualisierten
> Sicherheitsabhängigkeiten selbst. Die genaue Modulliste und Prüfsummen stehen
> in `scripts/rclone/go.mod`, `go.sum` und `build.json`; die jeweiligen
> Lizenzbedingungen gelten auch für diesen Build.

## 2. Bibliotheken, aus denen DualBeam erstellt wurde

Diese Bibliotheken sind in die Programmdatei einkompiliert. Ihre Lizenzen
verlangen, dass Urhebervermerk und Lizenztext der Weitergabe beiliegen.

### Rust

| Kiste | Version | Lizenz |
| --- | --- | --- |
| tauri | 2.12.7 | Apache-2.0 ODER MIT |
| tauri-plugin-drag | 2.1.1 | Apache-2.0 ODER MIT |
| serde | 1.0.229 | MIT ODER Apache-2.0 |
| serde_json | 1.0.154 | MIT ODER Apache-2.0 |
| dirs | 5.0.1 | MIT ODER Apache-2.0 |
| walkdir | 2.5.1 | Unlicense ODER MIT |
| trash | 5.4.0 | MIT |
| notify | 6.1.1 | CC0-1.0 |
| notify-debouncer-mini | 0.4.1 | MIT ODER Apache-2.0 |
| zip | 9.8.1 | MIT |
| libc | 0.2.191 | MIT ODER Apache-2.0 |
| url | 2.5.9 | MIT ODER Apache-2.0 |
| sha2 | 0.10.9 | MIT ODER Apache-2.0 |
| security-framework | 3.7.0 | MIT ODER Apache-2.0 |
| time | 0.3.55 | MIT ODER Apache-2.0 |
| tauri-plugin-updater | 2.14.1 | Apache-2.0 ODER MIT |

Die Angaben stammen aus dem `license`-Feld der jeweiligen `Cargo.toml` in der
lokalen Registry, nicht aus zweiter Hand. Mit den mittelbaren Abhängigkeiten
sind es einige Hundert Kisten; sie stehen mit wenigen Ausnahmen ebenfalls unter
MIT oder Apache-2.0. Die vollständige Liste lässt sich jederzeit erzeugen:

```sh
cargo tree --manifest-path src-tauri/Cargo.toml --prefix none --format '{p} {l}' | sort -u
```

### Weboberfläche

| Paket | Lizenz |
| --- | --- |
| SolidJS | MIT |
| @tauri-apps/api | Apache-2.0 ODER MIT |
| @crabnebula/tauri-plugin-drag | kein Lizenzfeld im npm-Paket — siehe Hinweis |

> **Hinweis:** Das npm-Paket `@crabnebula/tauri-plugin-drag` 2.1.0 enthält weder
> ein `license`-Feld noch eine Lizenzdatei. Es ist der JavaScript-Teil desselben
> Projekts wie die Rust-Kiste `tauri-plugin-drag` 2.1.1, und die nennt
> **Apache-2.0 ODER MIT**. Davon ist auszugehen; belegt ist es für das npm-Paket
> aber nicht. Sollte das je Bedeutung erlangen, ist beim Herausgeber
> (CrabNebula) nachzufragen.

Vite, TypeScript, Vitest und jsdom sind reine Bauwerkzeuge und werden nicht
mitgeliefert.

## 3. Werkzeuge des Betriebssystems

Nicht mitgeliefert, sondern nur aufgerufen — und deshalb ohne Lizenzpflicht für
DualBeam:

`curl`, `open`, `osascript`, `security`, `ssh-keygen`, `ssh-keyscan`,
`mount`, `umount`, `diskutil`, `hdiutil`, `mdfind`, `qlmanage`, `tmutil`.

## 4. Beigelegte Lizenztexte

Im Programmpaket unter `DualBeam.app/Contents/Resources/licenses/`:

| Datei | Inhalt |
| --- | --- |
| `DUALBEAM-LICENSE.txt` | MIT-Lizenz von DualBeam samt Urhebervermerk |
| `RCLONE-LICENSE.txt` | MIT-Lizenz von rclone samt Urhebervermerk |
| `MIT.txt` | Wortlaut der MIT-Lizenz |
| `APACHE-2.0.txt` | Wortlaut der Apache-Lizenz 2.0 |
| `THIRD-PARTY-LICENSES.txt` | diese Übersicht |

Für die beiden übrigen Lizenzen liegt bewusst kein eigener Text bei:
**CC0-1.0** (`notify`) ist eine Gemeinfreiheitserklärung und verlangt weder
Urhebervermerk noch Lizenztext. Bei **Unlicense ODER MIT** (`walkdir`) ist die
MIT-Alternative gewählt, deren Wortlaut in `MIT.txt` steht.
