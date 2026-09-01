# RemoteClientKit – technische Spezifikation

## Ziel

RemoteClientKit ist ein eigenständiger Remote-Desktop-Client für **macOS 26.0
oder neuer**. Er wird als signierte, notarisiertes `RemoteDesk.app` ausgeliefert
und kann später von DualBeam über ein stabiles, versioniertes Übergabeformat
gestartet werden. Das Modul enthält alle erforderlichen RDP-Komponenten;
eine Homebrew- oder sonstige Benutzerinstallation ist nicht erforderlich.

Dieses Dokument beschreibt das Verhalten des Clients. Welche Dateien in FreeRDP
und SDL dafür geändert wurden und warum, steht getrennt in
[FREERDP_PATCHES.md](FREERDP_PATCHES.md).

## Verbindungen

| Zielsystem | Protokoll | Voraussetzung auf dem Ziel |
|---|---|---|
| Windows | RDP | Remotedesktop aktiviert |
| Linux | RDP | RDP-Server, beispielsweise xrdp |

RDP verwendet, wenn verfügbar, UDP-Multitransport und fällt bei gesperrtem oder
nicht verfügbarem UDP automatisch auf TCP zurück. Video- bzw. Multimedia-
Weiterleitung gehört nicht zum zugesagten Funktionsumfang. Der Client nutzt sie
nur dann, wenn Server, Netzwerk und die gebündelten Codecs dies unterstützen.

## Ports

Alle Ports sind pro Profil änderbar:

| Einstellung | Vorgabe | Hinweis |
|---|---:|---|
| RDP TCP | 3389 | RDP-Steuer- und Fallback-Transport |
| RDP UDP | 3389 | optionaler, bevorzugter RDP-Transport |

## Wiederverbinden nach einem Aussetzer

Der Profilschalter **Wiederverbinden** (Abschnitt „Port & Netzwerk", Vorgabe
**an**) setzt FreeRDPs `+auto-reconnect`. Ohne ihn beendet FreeRDP die Sitzung
beim ersten verlorenen Takt endgültig: `AutoReconnectionEnabled` ist dort per
Vorgabe `FALSE` (`libfreerdp/core/settings.c`, Z. 1215). Mit ihm versucht der
Client bis zu zwanzigmal (`AutoReconnectMaxRetries`, ebenfalls vorbelegt), die
**bestehende** Sitzung fortzusetzen – angemeldete Programme, offene Fenster und
die Zwischenablage bleiben erhalten.

Wirksam wird das nur, wenn der Server beim Anmelden eine Wiederverbindungs-
Kennung ausgegeben hat (MS-RDPBCGR, `Server Auto-Reconnect Cookie`). Tut er das
nicht, bleibt der Schalter folgenlos; schaden kann er nicht.

**Häufigste Ursache echter Aussetzer sind angehaltene virtuelle Maschinen.**
Bei Parallels heisst die Einstellung „Bei Untätigkeit anhalten" (Konfigurieren ▸
Optionen ▸ Optimierung). Während einer RDP-Sitzung steht das Parallels-Fenster
nie im Vordergrund, die Maschine gilt daher als untätig und wird eingefroren.
Gemessen wurde dabei eine TCP-Bilanz ohne jedes Abbausignal der Gegenstelle
(`FIN in/out: 0/0`), nur sieben Neuübertragungen bis zum Zeitablauf nach 68 s.
Abschalten mit `sudo prlctl set "<VM-Name>" --pause-idle off`.

## Sitzungsprotokolle

Jede Sitzung schreibt FreeRDPs Fehlerausgabe nach
`~/Library/Application Support/RemoteDesk/logs/<profil-id>.log`. Die Datei
beginnt mit dem Startzeitpunkt in Ortszeit und endet mit dem Rückgabewert des
Backends – beides ist nötig, um eine Zeile mit dem Systemprotokoll
(`log show`) zusammenzuführen, denn FreeRDPs eigene Ausgabe trägt keine Uhrzeit.

Beim Start wird **nicht** überschrieben, sondern verschoben: `…log` wird zu
`…log.1`, `…log.1` zu `…log.2` und so fort bis `…log.5`. Grund: Nach einem
Abbruch verbindet man sofort neu – und löschte damit vorher genau das
Protokoll, das den Grund enthielt.

Der Client setzt `/log-level:INFO` ausdrücklich. Gemessen ist INFO bereits
FreeRDPs Vorgabe – die Ausgabe ist mit und ohne Schalter identisch. Er hält den
Umfang lediglich fest, falls eine spätere Fassung die Vorgabe absenkt.
Gebraucht werden genau zwei INFO-Zeilen: `Network disconnect!` und `Attempting
reconnect (n of 20)` (`client/common/client.c`). Stehen sie **nicht** im
Protokoll, gab es weder einen Netzabbruch noch einen Wiederverbindungsversuch –
das ist beim Deuten eines Abbruchs die halbe Antwort.

### Einen Abbruch lesen

Die Kennung in eckigen Klammern ist `[PID:Thread]`. Sie verrät, **woher** der
Abbruch kam, und das ist meist die entscheidende Frage:

| Thread | Bedeutung |
|---|---|
| derselbe wie die Verbindungsmeldungen | RDP-Thread – der Abbruch kam von aussen (Netz, Server) |
| derselbe wie `[handleShow]` | SDL-Hauptthread – der Abbruch kam von innen (Fenster geschlossen, ⌘Q) |

Ergänzend zeigt der Kernel für jede beendete TCP-Verbindung eine Bilanz:

```
log show --last 30m --predicate 'process == "kernel"' | grep -E "3389|rtt:"
```

| Muster | Deutung |
|---|---|
| `tcp_usrclosed`, `FIN in/out: 0/0` | der Prozess baute sauber ab |
| `tcp_drop`, `so_error: 60`, `rxmit > 0` | Gegenstelle verstummte – echter Ausfall oder angehaltene VM |
| `tcp_drop`, `RST out: 1`, `rxmit: 0` | der Prozess endete, während noch Daten anlagen |
| `RST in: 1` | die Gegenstelle setzte zurück |

## Fenstergröße und Auflösung

Jedes RDP-Profil legt fest, wie sich das Sitzungsfenster verhält:

| Einstellung | Vorgabe | Wirkung |
|---|---|---|
| Fenstermodus | Fenster | `Fenster` startet mit fester Auflösung, `Nutzbare Bildschirmfläche` füllt den Arbeitsbereich, `Vollbild` belegt den ganzen Bildschirm |
| Breite × Höhe | 1600 × 1000 | Startauflösung im Fenstermodus, gerade Werte ab 640 × 480 |
| Verhalten beim Vergrößern | Auflösung mitziehen | `Auflösung mitziehen` ändert die Serverauflösung, `Bild skalieren` skaliert das Bild, `Feste Auflösung` lässt das Fenster starr |

Die Farbtiefe wird als `/bpp` übergeben. FreeRDP akzeptiert ausschliesslich 32,
24, 16, 15 und 8 Bit; jeder andere Wert führt zum Argumentfehler. Ohne Auswahl
entfällt die Option, dann handeln Client und Server die Farbtiefe selbst aus.
Weniger Bit bedeuten weniger Daten und helfen auf schmalen Leitungen.

## Ordnerfreigabe

Im Abschnitt „Arbeitsumgebung“ lassen sich beliebig viele Ordner des Macs in die
Sitzung reichen. Jeder Eintrag wird als `/drive:Name,Pfad` übergeben und
erscheint im Gastsystem unter den umgeleiteten Laufwerken – unter Windows im
Explorer neben den lokalen Laufwerken, unter Linux je nach Sitzung im
Dateimanager.

Der Name ist frei wählbar und wird beim Hinzufügen aus dem Ordnernamen
vorbelegt. Ein Komma ist nicht erlaubt: FreeRDP trennt damit Name und Pfad und
kennt keine Maskierung. RemoteDesk weist solche Einträge beim Verbinden mit einer
Meldung ab, statt still eine falsche Freigabe anzulegen.

## Zwischenablage

Das gemeinsame Kopieren in beide Richtungen setzt `+clipboard` voraus (Schalter
„Zwischenablage freigeben“). macOS meldet Änderungen der Zwischenablage nicht
über ein Ereignis; SDL vergleicht deshalb den `changeCount` des Pasteboards.
Im Original tat es das ausschliesslich beim Fokuswechsel auf das Fenster –
was ausserhalb kopiert wurde, erreichte die Sitzung nie, solange das
RDP-Fenster den Fokus behielt. Der mitgelieferte SDL-Patch prüft zusätzlich in
der Ereignisschleife, gedrosselt auf höchstens alle 250 ms, damit der Renderpfad
unbelastet bleibt.

### Dateien kopieren

Kopierte Dateien gelangen vom Mac in die Sitzung, sofern der Server die
Dateiübertragung anbietet. macOS legt sie als `public.file-url` ab, einen
Eintrag je Datei; FreeRDP erwartet dagegen `text/uri-list`. Der SDL-Patch setzt
diesen Typ aus allen Einträgen zusammen, wodurch auch eine Mehrfachauswahl
vollständig übertragen wird. Der FreeRDP-Patch meldet dem Server die
Dateiunterstützung und bietet `FileGroupDescriptorW` an.

Eine Besonderheit des Finders kommt hinzu: Er legt keine Pfad-URLs ab, sondern
File-Reference-URLs der Form `file:///.file/id=6571367.121757495`. Wörtlich
gelesen ergibt das den Pfad `/.file/id=…`, den es nicht gibt — der Empfänger
scheitert dann still beim `stat()` und der Gast bietet das Einfügen zwar an,
tut aber nichts. Der SDL-Patch löst deshalb jede Datei-URL über `filePathURL`
in den echten Pfad auf.

Beide Richtungen funktionieren inzwischen, ebenso Ziehen und Ablegen (siehe
unten). Meldet der Server keine Dateiunterstützung, bleibt das Kopieren auf Text
und Bilder beschränkt. Bei xrdp kam das vor, wenn eine ältere Sitzung
wiederverwendet wurde; nach einer Abmeldung im Gast bot derselbe Server die
Dateiübertragung wieder an.

### Ordner: Grenze des Servers, nicht des Clients

Einzelne Dateien laufen gegen jedes Ziel, Ordner dagegen nur gegen Windows.
Gegen xrdp scheitern sie in **beiden** Richtungen. Die Ursache liegt in xrdp,
nachlesbar in `sesman/chansrv/clipboard_file.c`:

- Richtung Mac → Sitzung (`clipboard_c2s_in_files`, Z. 622): jeder Eintrag mit
  gesetztem `CB_FILE_ATTRIBUTE_DIRECTORY` **oder** einem Backslash im Namen wird
  per `continue` verworfen — „skipping directory not supported“.
- Richtung Sitzung → Mac (`clipboard_get_file`, Z. 171): `g_directory_exist()`
  führt zu `return 1`, der Ordner kommt gar nicht erst in die Liste — „is a
  directory, not supported“.

Bei einem Ordner mit Unterordner fällt damit die **gesamte** Liste weg: das
Verzeichnis wegen des Attributs, alle enthaltenen Dateien wegen des Backslashs.
Der Gast erhält null verwertbare Einträge und meldet einen leeren Dateinamen
(„Fehler beim Einlesen der Informationen über »«“) samt „Vorgang wird nicht
unterstützt“ — was `G_IO_ERROR_NOT_SUPPORTED` in GIO entspricht.

Gemessen und gegengeprüft: RemoteDeskRDP sendet für `RDP-Ordnertest` fünf
korrekte Deskriptoren (Verzeichnisse mit `attr=0x10`, Unterpfade
Backslash-getrennt), wie es MS-RDPECLIP §2.2.5.2.3.1 vorsieht. Gegen Windows 11
kommen Ordner in beiden Richtungen vollständig an. Der Client ist also sauber;
die Grenze ist serverseitig und clientseitig nicht behebbar.

Für Ordner auf Linux-Zielen bleibt die **Ordnerfreigabe** (`/drive:`), die
diesen Weg nicht nimmt, oder ein Archiv.

#### Sonderzeichen in Ordnernamen

Die Dateiliste reist als URI-Liste, also prozentkodiert: aus `Mein Ordner` wird
`Mein%20Ordner`. FreeRDP dekodierte das nur an einer von drei Stellen, weshalb
die Verzeichnisprüfung auf einem Namen arbeitete, den es im Dateisystem nicht
gibt. Ordner mit **Leerzeichen, Umlaut, `%` oder `#`** galten dadurch nicht als
Verzeichnis und wurden ohne ihren Inhalt übertragen — schlichte Namen liefen
zufällig richtig. Der FreeRDP-Patch dekodiert jetzt genau einmal, bevor
irgendetwas den Pfad benutzt (Punkt 10 des Patches).

Zwei Einschränkungen bleiben darüber hinaus:

- **Grosse Dateien blockieren beim Einfügen.** Die Gegenrichtung lädt die
  Dateien vollständig herunter, bevor der Finder sie erhält.
- **Nur der erste Ziehvorgang je Auswahl.** macOS kann einen Ziehvorgang nur
  beginnen, solange die Maustaste gedrückt ist.

Das Mitziehen der Auflösung benötigt den Display-Control-Kanal. Der
mitgelieferte SDL-Client beherrscht ihn, der native Cocoa-Client nicht.
RemoteDesk schaltet auf dem Cocoa-Client automatisch auf die Skalierung um,
damit das Fenster frei veränderbar bleibt; Server ohne den Kanal verhalten sich
genauso. Beide Optionen schließen sich in FreeRDP gegenseitig aus und werden
nie gemeinsam übergeben.

Das RDP-Backend wird in dieser Reihenfolge gesucht: `REMOTEDESK_RDP_EXECUTABLE`,
das mitgelieferte `sdl-freerdp`, der mitgelieferte native Client `MacFreeRDP`,
ein per Homebrew installiertes `sdl-freerdp` und zuletzt `xfreerdp` mit XQuartz.

Der SDL-Client ist die erste Wahl, weil nur er die Auflösung wirklich mitzieht.
Er darf jedoch nicht mit seinem Standard-Renderer laufen: Metal blockiert unter
macOS in `[CAMetalLayer nextDrawable]`, die Ereignisschleife verhungert und die
Sitzung friert mit dem macOS-Wartekreisel ein. RemoteDesk erzwingt deshalb
`SDL_RENDER_DRIVER=opengl`.

Zusätzlich ist der SDL-Client gepatcht. Im Original stellt er je eingegangenem
Aktualisierungspaket ein Bild dar, und jedes `SDL_RenderPresent` wartet auf den
Bildwechsel des Monitors. Ein Bildaufbau aus zwanzig Paketen kostete so zwanzig
Wartezyklen — rund ein Drittel einer Sekunde. Der Patch sammelt alle
anstehenden Pakete und stellt danach einmal dar. Nebenbei entfiel eine
überflüssige Vollbildauffrischung, die das Original bei jedem Durchlauf zum
Beenden der Schleife zeichnete.

FreeRDP markiert den Cocoa-Client beim Start als „deprecated" und verweist auf
den SDL3-Client. Diese Warnung ist bekannt und wird bewusst in Kauf genommen:
Der SDL3-Client ist unter macOS 26 nicht bedienbar. Fällt der Cocoa-Client in
einer künftigen FreeRDP-Version weg, muss zuvor geprüft werden, ob der
SDL3-Client die Metal-Blockade nicht mehr zeigt.

## Gateway (RD Gateway)

Ein RD-Gateway nimmt die Verbindung von außen entgegen und reicht sie im
Firmennetz weiter; RDP läuft dabei getunnelt über HTTPS, daher der Standardport
443. Der Schalter „Über ein Gateway verbinden“ baut daraus `/gateway:`.

Bleiben Benutzer, Domäne und Kennwort leer, wird bewusst **nur** `g:` gesendet.
FreeRDP setzt dann in `parse_gateway_host_option()` das Kennzeichen
`GatewayUseSameCredentials = TRUE` und meldet das Gateway mit den Anmeldedaten
der Sitzung an — der übliche Fall. Sobald eines der Felder gefüllt ist, sendet
RemoteDeskRDP `u:`/`d:`/`p:`, und `parse_gateway_cred_option()` schaltet dasselbe
Kennzeichen auf FALSE. Ein leeres `u:` mitzusenden würde diesen Rückfall also
stillschweigend abschalten; ein Test hält das fest.

Das Gatewaykennwort liegt unter einem **eigenen** Schlüsselbunddienst
(`com.nojan.remotedesk.gateway`) neben dem Sitzungskennwort. Der bestehende
Dienst durfte dafür nicht angetastet werden — daran hängen die bereits
gespeicherten Kennwörter.

### Warum die Werte maskiert werden

`/gateway:` wird an Kommas zerlegt; erst danach entfernt `unescape()`
(`client/common/cmdline.c`) die Maskierungen. Am gebauten Binary gemessen:

| Eingabe im Kennwort | Ergebnis |
|---|---|
| `ge,heim` | `Command line parsing failed` |
| `ge\,heim` | angenommen |
| `ge"heim` | **Gateway stillschweigend verworfen** |
| `ge\"heim` | angenommen |
| `ge\heim` | angenommen, aber der Backslash fällt weg |

Der dritte Fall ist der gefährliche: Die Zerlegung bricht am Anführungszeichen
ab, `parse_gateway_options()` steigt bei `if (count == 0) return TRUE;` aus, und
die Verbindung geht **direkt** zum Ziel statt über das Gateway. Nachweisbar am
Fehlerbild — statt `DNS_NAME_NOT_FOUND` für den Gatewaynamen erscheint
`CONNECT_FAILED` für die Zieladresse.

`escape_gateway_value()` maskiert deshalb Backslash, Komma und beide
Anführungszeichen. Ein Rust-Test bildet FreeRDPs `unescape()` nach und prüft,
dass der Rundlauf den Ausgangswert unverändert wiederherstellt; zusätzlich wurde
ein von der App erzeugtes Argument mit allen vier Zeichen gegen das echte Binary
geprüft.

Nicht angeboten werden Transportwahl (`type:rpc|http|arm`) und Azure Virtual
Desktop. `auto` deckt RPC und HTTP ab; ARM braucht zusätzlich eine Token-URL und
gehört zu einem anderen Anmeldeverfahren. Kerberos ist im Backend nicht gebaut
(`WITH_KRB5=OFF`), die Gatewayanmeldung läuft über NTLM.

## Druckerfreigabe

Der Profilschalter „Drucker freigeben" hängt `/printer` an den Aufruf. Ohne
weiteres Argument reicht FreeRDP damit **alle** über CUPS eingerichteten
Drucker weiter; eine Auswahl einzelner Geräte gibt es bewusst nicht, weil bei
einer typischen Arbeitsplatzeinrichtung ohnehin nur wenige Warteschlangen
bestehen.

Die Voraussetzung ist bereits erfüllt: Das mitgelieferte Backend wurde mit
`WITH_CUPS=ON` übersetzt (`CHANNEL_PRINTER_CLIENT=ON`), und
`libfreerdp-client3` ist gegen `/usr/lib/libcups.2.dylib` gebunden — nachprüfbar
mit

```sh
otool -L …/MacFreeRDP.app/Contents/Frameworks/libfreerdp-client3*.dylib | grep cups
```

Es war dafür keine Änderung am Bauskript nötig; CMake findet CUPS über das
Xcode-SDK von allein.

Ob ein Ausdruck ankommt, entscheidet die Gegenstelle: Sie braucht zum
gemeldeten Gerät einen Treiber. FreeRDP übergibt den CUPS-Treibernamen, sofern
keiner mit `/printer:<name>,<treiber>` erzwungen wird. Die Druckerliste wird
beim Verbindungsaufbau ermittelt; spätere Änderungen wirken erst in einer neuen
Sitzung.

## Smartcard und Videowiedergabe

Zwei weitere Profilschalter reichen Kanäle durch, die das Backend bereits
mitbringt:

- **„Smartcard freigeben"** hängt `/smartcard` an. Der Kanal ist mit
  `WITH_PCSC=ON` und `CHANNEL_SMARTCARD=ON` gebaut. Anders als unter Linux ist
  kein `pcscd` einzurichten: `libwinpr3` lädt
  `/System/Library/Frameworks/PCSC.framework/PCSC` zur Laufzeit per `dlopen` —
  die Bibliothek ist deshalb auch nicht in `otool -L` zu sehen, wohl aber als
  Zeichenkette im Binärcode.
- **„Videowiedergabe beschleunigen"** hängt `/video` an (MS-RDPEVOR). Der
  Server darf Videoinhalte dann als H.264-Strom senden. Die Dekodierung
  übernimmt das mitgelieferte `libopenh264.8.dylib` (`WITH_OPENH264=ON`);
  `WITH_FFMPEG` ist aus und wird dafür nicht gebraucht.

### Warum es keine Webcam-Weiterleitung gibt

Der dafür zuständige Kanal MS-RDPECAM ist zwar vorhanden, aber nur als
Server-Teil: `CHANNEL_RDPECAM_CLIENT:BOOL=OFF`. Das hat einen sachlichen Grund —
FreeRDP liefert unter `channels/rdpecam/client/` genau ein Backend mit, nämlich
`v4l` (Video4Linux). Ein AVFoundation-Backend für macOS existiert nicht, der
Kanal hätte hier also nichts anzusteuern.

Der Umweg über die USB-Weiterleitung (`/usb:id,dev:<vid>:<pid>`) ist gebaut —
`CHANNEL_URBDRC=ON`, `libusb-1.0.dylib` liegt im Bundle — führt bei Kameras aber
kaum zum Ziel: macOS bindet Geräte der USB-Video-Klasse über CoreMediaIO
exklusiv ein, und libusb kann sie dem Systemtreiber nicht ohne Weiteres
entziehen. Deshalb gibt es dafür bewusst keinen Schalter.

Zu beachten: `/video` ist **keine** Kameraweiterleitung, auch wenn der Name das
nahelegt. Es beschleunigt ausschliesslich Video, das im Gastsystem abgespielt
wird.

## Erscheinungsbild und Sprache

Im Fuß der Seitenleiste sitzen zwei kleine Schalter.

**Thema** wechselt zwischen `Automatisch` (◐), `Hell` (☀︎) und `Dunkel` (☾).
Automatisch folgt der Systemeinstellung von macOS und reagiert auf einen
Wechsel im laufenden Betrieb, ohne dass die App neu gestartet werden muss.

**Sprache** wechselt zwischen `Automatisch` (⚙), `Deutsch` (DE) und
`English` (EN). Automatisch richtet sich nach der Browsersprache des
Systems; alles außer Deutsch ergibt Englisch.

Beide Einstellungen gelten für die ganze App, nicht je Profil, und liegen
unter den Schlüsseln `remotedesk:theme:v1` und `remotedesk:lang:v1` im
lokalen Speicher.

**Das Sitzungsfenster schaltet nicht mit.** Es wird von FreeRDP und vom
Gastsystem gezeichnet – Thema und Sprache stellt man dort im Gast ein.

Auch Fehlermeldungen des Backends folgen der Sprache: Das Backend liefert
keinen fertigen Satz mehr, sondern einen Code wie `err.hostRequired`, den die
Oberfläche nachschlägt. Ein unbekannter Code bleibt unverändert stehen, damit
nie eine leere Meldung erscheint.

## Sicherheitsmodell

- Kennwörter werden ausschließlich im macOS-Schlüsselbund gespeichert.
- Ein Profil enthält niemals ein Kennwort.
- Zertifikate werden standardmäßig per TOFU (beim ersten Kontakt vertrauen,
  bei späterer Änderung ablehnen) über FreeRDP geprüft; ein strikter
  Ablehnungsmodus ist pro Profil verfügbar.
- Keine Passwörter in Kommandozeilen, Logs oder DualBeam-Übergabeparametern.
- Beim Löschen eines Profils entfernt `forget_passwords()` beide zugehörigen
  Einträge (`com.nojan.remotedesk.password` und `…gateway`). Das geschieht erst
  **nach** dem erfolgreichen Schreiben der Profildatei: schlägt das Schreiben
  fehl, bleibt das Profil bestehen und behält sein Kennwort. Ein fehlender
  Eintrag gilt nicht als Fehler — ein Profil ohne Gatewaykennwort ist der
  Normalfall, und ein zweiter Löschversuch darf nicht stolpern.

## Fehlersuche

**Windows meldet „Logon failed", obwohl Benutzername und Kennwort stimmen.**
Windows verweigert Netzwerkanmeldungen mit leerem Kennwort; die Voreinstellung
`HKLM\SYSTEM\CurrentControlSet\Control\Lsa\LimitBlankPasswordUse` steht auf `1`.
Ein Konto ohne Kennwort kann sich also lokal anmelden, per RDP aber nicht. Im
Gast ein Kennwort setzen (`Einstellungen → Konten → Anmeldeoptionen`). Achtung:
`Get-LocalUser` zeigt in diesem Fall trotzdem ein `PasswordLastSet`-Datum – das
stammt von der Installation und beweist kein gesetztes Kennwort. Aussagekräftig
ist `PasswordRequired: False`.

Weiter gilt: Das Konto muss Administrator oder Mitglied der Gruppe
*Remotedesktopbenutzer* sein.

**Das Kennwort wird bei jeder Verbindung erneut abgefragt.** Die Abfrage im
Sitzungsfenster stammt von FreeRDP und speichert nichts. Damit es im
Schlüsselbund landet, gehört es in das Feld *Passwort* des Profils, gefolgt von
*Speichern*.

## DualBeam-Integration

DualBeam benötigt zunächst keine Plugin-Schnittstelle. Die Schnittstelle ist
ein versionierter JSON-Aufruf an die Standalone-App:

```json
{
  "version": 1,
  "action": "open-profile",
  "profileId": "7f7289a7-6cd1-4f6a-9597-0f4fde8bd71d"
}
```

Später kann DualBeam denselben Aufruf über einen Rust-Adapter statt über den
Starter-Prozess ausführen. Das Profilformat und die Schlüsselbund-Zuordnung
bleiben dabei unverändert.

## Verpackung

Die Release-App enthält ein universelles (Apple Silicon und Intel) RDP-Backend
inklusive dessen dynamischer Bibliotheken. Vor dem öffentlichen
Release erfolgen Code-Signing und Notarization. FreeRDP-Lizenzhinweise werden
dem Bundle beigefügt.

## Lizenz

Die App zeigt unterhalb der Hilfe den Punkt **§ Lizenz**. Er enthält vier
Abschnitte: die Endbenutzer-Lizenzvereinbarung, die Liste aller mitgelieferten
Module mit ihren Lizenzen, den Hinweis zur Quelltextbereitstellung und die
Aufstellung der geänderten Dateien in FreeRDP und SDL.

Der Text steht in beiden Wörterbüchern (`src/locale/de.ts`, `src/locale/en.ts`)
unter dem Präfix `license.`; die Reihenfolge der Abschnitte legt
`licenseSections` in `App.tsx` fest — gleiche Bauart wie bei der Hilfe.

Ergänzt wurde eine **Ziffer 5 (Vorrang der Fremdlizenzen)**. Ohne sie hätten das
Reverse-Engineering-Verbot in Ziffer 3 und die Beschränkung auf unveränderte
Weitergabe in Ziffer 4 den Lizenzen der mitgelieferten Bibliotheken
widersprochen — namentlich der GNU LGPL 2.1 von FFmpeg und libusb, die dem
Empfänger genau diese Rechte einräumen muss. Einzelheiten in
[../remote-client/EULA.md](../remote-client/EULA.md) und
[../remote-client/THIRD_PARTY_LICENSES.md](../remote-client/THIRD_PARTY_LICENSES.md).

`scripts/build-freerdp-backend.sh` legt die vollständigen Lizenztexte im
Programmpaket ab (`resources/freerdp/licenses/`, dazu `FREERDP-LICENSE.txt`).
Das ist keine Kür: Apache-2.0, LGPL, BSD, zlib und die Fraunhofer-Lizenz
verlangen alle, dass ihr Text der Binärfassung beiliegt.
