# Verwendete Fremdkomponenten und deren Lizenzen — DualBeam

Stand: 1. August 2026, DualBeam 0.4.0

DualBeam selbst steht unter der [MIT-Lizenz](LICENSE). Für die hier aufgeführten
Fremdkomponenten gelten zusätzlich deren eigene Lizenzen.

---

## 1. Mitgeliefertes Programm

Genau ein fremdes Programm wird als Datei mitgeliefert:

| Komponente | Version | Lizenz | Urheber |
| --- | --- | --- | --- |
| [rclone](https://rclone.org) | 1.74.4 | MIT | Nick Craig-Wood und Mitwirkende |

rclone liegt im Programmpaket unter `DualBeam.app/Contents/MacOS/rclone` und
stellt SFTP- und FTPS-Verbindungen als Laufwerk bereit. Der vollständige
Lizenztext liegt bei (`RCLONE-LICENSE.txt`).

> rclone ist in Go geschrieben und enthält seinerseits zahlreiche
> Open-Source-Bibliotheken unter eigenen Lizenzen. Deren Auflistung ist Teil des
> rclone-Projekts und dort einsehbar; sie werden durch die Weitergabe der
> unveränderten rclone-Binärdatei nicht berührt.

## 2. Bibliotheken, aus denen DualBeam erstellt wurde

Diese Bibliotheken sind in die Programmdatei einkompiliert. Ihre Lizenzen
verlangen, dass Urhebervermerk und Lizenztext der Weitergabe beiliegen.

### Rust

| Kiste | Version | Lizenz |
| --- | --- | --- |
| tauri | 2.11.2 | Apache-2.0 ODER MIT |
| tauri-plugin-drag | 2.1.1 | Apache-2.0 ODER MIT |
| serde | 1.0.228 | MIT ODER Apache-2.0 |
| serde_json | 1.0.149 | MIT ODER Apache-2.0 |
| dirs | 5.0.1 | MIT ODER Apache-2.0 |
| walkdir | 2.5.0 | Unlicense ODER MIT |
| trash | 5.2.6 | MIT |
| notify | 6.1.1 | CC0-1.0 |
| notify-debouncer-mini | 0.4.1 | MIT ODER Apache-2.0 |
| zip | 2.4.2 | MIT |
| libc | 0.2.186 | MIT ODER Apache-2.0 |
| url | 2.5.8 | MIT ODER Apache-2.0 |
| sha2 | 0.10.9 | MIT ODER Apache-2.0 |
| security-framework | 3.7.0 | MIT ODER Apache-2.0 |
| objc2 | 0.6.4 | MIT |

Die Angaben stammen aus dem `license`-Feld der jeweiligen `Cargo.toml` in der
lokalen Registry, nicht aus zweiter Hand. Mit den mittelbaren Abhängigkeiten
sind es einige Hundert Kisten; sie stehen mit wenigen Ausnahmen ebenfalls unter
MIT oder Apache-2.0. Die vollständige Liste lässt sich jederzeit erzeugen:

```sh
cargo tree --prefix none --format '{p} {l}' | sort -u
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
