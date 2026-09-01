# Änderungen an FreeRDP und SDL

RemoteDeskRDP liefert FreeRDP mit, baut es aber nicht unverändert. Zwei Patches
greifen ein, beide liegen in `remote-client/scripts/patches/`:

| Patch | Ziel | Dateien |
|---|---|---|
| `freerdp-3.26.0-macos-client.patch` | FreeRDP, Tag `3.26.0` | 12 |
| `sdl-3.2.28-clipboard-poll.patch` | SDL, Tag `release-3.2.28` | 3 |

`scripts/build-freerdp-backend.sh` setzt die betroffenen Dateien zuerst per
`git checkout --` zurück und wendet dann den FreeRDP-Patch an — ein
halb zurückgenommener Baum kann so kein stillschweigend ungepatchtes Ergebnis
liefern. Den SDL-Patch reicht das Skript über die Umgebungsvariable
`REMOTEDESK_SDL_PATCH` an FreeRDPs `scripts/bundle-mac-os.sh` weiter, das ihn
nach dem Klonen anwendet.

Die Patchköpfe selbst gliedern nach Thema. Dieses Dokument gliedert nach
**Datei** und beantwortet je Datei: was wurde geändert und warum.

---

## Teil A — FreeRDP 3.26.0

### `scripts/bundle-mac-os.sh` (+11 Zeilen)

**Was:** Ergänzt `-DWITH_CLIENT_MAC=ON` in den CMake-Argumenten.

**Warum:** Das Bundling-Skript baut den nativen Cocoa-Client nicht mit. Ohne
diese Zeile entsteht nur `sdl-freerdp`, und `MacFreeRDP` stünde nicht als
Rückfall zur Verfügung.

### `client/Mac/Keyboard.m` (12 Zeilen)

**Was:** Korrigiert `mac_detect_keyboard_type()`.

**Warum:** Reiner Baufehler. Er tritt erst zutage, sobald der Cocoa-Client
überhaupt gebaut wird — upstream fällt er deshalb nicht auf.

### `client/Mac/cli/MainMenu.xib` (3 Zeilen)

**Was:** Entfernt die Obergrenze der Fenstergröße und senkt die Untergrenze auf
640×480.

**Warum:** Das Fenster war auf 1024×768 festgenagelt — `minSize` war gleich
`maxSize`. Ein Vergrößern war schlicht unmöglich.

### `client/Mac/cli/AppDelegate.h` (+1 Zeile)

**Was:** Deklariert `mac_set_view_size(rdpContext*, MRDPView*)`.

**Warum:** Die Funktion wird jetzt aus `AppDelegate.m` heraus verwendet und
braucht eine sichtbare Deklaration.

### `client/Mac/cli/AppDelegate.m` (40 Zeilen)

**Was:** Hängt die Sitzungsansicht mit Autoresizing-Maske ein und hält in
`sessionViewDidResize:` `client_width`/`client_height` nach.

**Warum:** Die Ansicht wuchs nie mit dem Fenster mit. Zusätzlich ruft der
Kommandozeilen-Client `setScrollOffset:y:w:h:` nie auf, weshalb Skalierung und
Mauskoordinaten ohne diese Nachführung toter Code blieben.

### `client/SDL/SDL3/sdl_freerdp.cpp` (18 Zeilen)

**Was:** Die Bildschleife leert die Warteschlange erst vollständig
(`drawToWindows(rects, false)`) und stellt danach genau einmal dar
(`presentWindows()`).

**Warum:** Vorher stellte der Client je eingegangenem Aktualisierungspaket ein
Bild dar. Jedes `SDL_RenderPresent` wartet auf den Bildwechsel des Monitors —
ein Bildaufbau aus zwanzig Paketen kostete zwanzig Wartezyklen. Außerdem
zeichnete die Schleife bei jedem Durchlauf eine überflüssige
Vollbildauffrischung, weil ein leeres Paket als „alles neu zeichnen" ausgelegt
wurde. Gemessen sank die Renderlast im Hauptthread von 47 auf 1 Prozent.

### `client/SDL/SDL3/sdl_context.cpp` (245 Zeilen)

**Was:** Drei Bereiche.

1. `drawToWindow`/`drawToWindows` bekommen einen Schalter, ob dargestellt
   werden soll; `presentWindows()` kommt hinzu (Gegenstück zur Änderung in
   `sdl_freerdp.cpp`).
2. `handleEvent(SDL_DropEvent)` sammelt abgelegte Dateien, statt sie zu
   verwerfen, und übergibt sie beim Loslassen an den Zwischenablage-Pfad.
3. `handleEvent(SDL_MouseButtonEvent/SDL_MouseMotionEvent)` erkennt das
   Herausziehen aus dem Fenster; `pixelToScreen`, `useLocalScale` und
   `updateMonitorDataFromOffsets` rechnen die Koordinaten um.

**Warum:** RDP hat keinen Kanal für Ziehen und Fallenlassen — die Kanalliste
kennt nur `cliprdr` und `rdpdr`. Der einzige Weg führt über die Zwischenablage.
Die Koordinatenumrechnung muss dieselbe Kette nehmen wie echte Mausereignisse
(`eventToPixelCoordinates`, `removeLocalScaling`, `applyMonitorOffset`), sonst
zeigt der Gastzeiger bei skalierten Fenstern woandershin.

### `client/SDL/SDL3/sdl_context.hpp` (23 Zeilen)

**Was:** Deklarationen zu den obigen Änderungen in `class SdlContext`.

**Warum:** Neue Methoden und Zustandsfelder für Darstellung und Ziehvorgang.

### `client/SDL/SDL3/sdl_clip.cpp` (273 Zeilen)

Die umfangreichste Datei auf der SDL-Client-Seite, vier voneinander unabhängige
Gründe:

**1. Leere Formatliste beim Verbindungsaufbau.** Beim Aushandeln des
Zwischenablage-Kanals stößt `MonitorReady` ein Ereignis ohne Formate an. Der
Client meldete daraufhin eine leere Liste, sodass alles vor dem
Verbindungsaufbau Kopierte im Gast unerreichbar blieb. `handleEvent` lädt den
Zustand der Systemzwischenablage jetzt nach, wenn ein Ereignis keine Formate
mitbringt.

**2. Kopierte Dateien erreichten die Sitzung nie.** Es fehlten zwei Dinge: Der
Client rief `cliprdr_file_context_set_locally_available()` nie auf, weshalb
`cliprdr_file_context_current_flags()` dauerhaft `0` lieferte und der Server die
Dateiunterstützung nicht erfuhr; und die Formatliste kannte `text/uri-list`
nicht, sodass `FileGroupDescriptorW` nie angeboten wurde. Der X11-Client
verfährt in `xf_cliprdr.c` genauso.

**3. Gegenrichtung (Sitzung → Mac).** `ClipDataCb` wartet vor dem Einfügen über
`cliprdr_file_context_wait_for_files()` auf die fertig abgeholten Dateien.
Warten darf nur der SDL-Thread — ein wartender Kanal-Thread könnte die
Antworten des Servers nicht mehr annehmen und liefe in die Zeitsperre.

**4. Ziehen und Fallenlassen.** `offerDroppedFiles()` kündigt dem Gast
`FileGroupDescriptorW` an, ohne die Mac-Zwischenablage anzutasten: Die Pfade
liegen in `_drop_uri_list` und werden in `ReceiveFormatDataRequestHandle`
bevorzugt vor `SDL_GetClipboardData` bedient. `waitForDropAck()` wartet auf das
Ereignis aus `ReceiveFormatListResponse`, denn erst nach der Bestätigung liegen
die Dateien in der Zwischenablage des Gasts — ein sofortiges Strg+V ginge ins
Leere. In der Gegenrichtung holt `fetchServerFiles()` die Dateien über den
internen Typ `application/x-remotedeskrdp-files` ab.

### `client/SDL/SDL3/sdl_clip.hpp` (31 Zeilen)

**Was:** Deklarationen zu den vier obigen Punkten in `class sdlClip`.

**Warum:** Neue Methoden (`offerDroppedFiles`, `waitForDropAck`,
`fetchServerFiles`) und die Felder für `_drop_uri_list` und das Bestätigungs-
Ereignis.

### `client/common/client_cliprdr_file.c` (507 Zeilen)

Die größte Änderung überhaupt, und die einzige Datei, die **zwei
grundverschiedene Dinge** enthält: eine Erweiterung und einen echten Fehler von
FreeRDP.

#### A) Dateien aus der Sitzung holen, ohne FUSE (Erweiterung)

FreeRDP schaltet die Richtung Sitzung → Mac auf Apple ausdrücklich ab:

```
client/common/CMakeLists.txt
  if(NOT APPLE AND NOT WIN32 AND NOT ANDROID)  →  OPT_FUSE_DEFAULT ON
  sonst OFF, und pkg_check_modules(FUSE3 REQUIRED fuse3)
```

Ohne `WITH_FUSE` gibt `cliprdr_file_context_has_local_support()` hart `FALSE`
zurück, `cliprdr_file_context_update_server_data()` ist vollständig leer, und
`ServerFileContentsResponse` wird gar nicht erst registriert. Es fehlte also
nicht die Freigabe, sondern die Umsetzung.

macFUSE scheidet aus (fremde Kernel-Erweiterung, nicht mitlieferbar), FUSE-T
ebenfalls: dessen API weicht nach eigener Aussage von der Linux-Fassung ab, die
FreeRDP verwendet (`FUSE_USE_VERSION 30`, `fuse_lowlevel.h`), und die Lizenz ist
nur für nicht-kommerzielle Nutzung frei.

Der Ausweg steckt in `winpr/libwinpr/clipboard/synthetic_file.c` (Z. 726–800):
Der Synthesizer baut die Zeilen als `file://<delegate->basePath>/<Name>` und
**prüft nicht, ob die Dateien existieren**. Es genügt also, sie wirklich
herunterzuladen und `basePath` auf ihren Ordner zu setzen. Umgesetzt in einem
eigenen `#if !defined(WITH_FUSE)`-Block:

- `update_server_data()` liest die Liste und legt `clip-<n>` unter `file->path` an
- ein eigener Thread fragt je Datei `FILECONTENTS_SIZE`, dann `FILECONTENTS_RANGE`
  in 1-MB-Stücken; immer nur eine Anfrage offen, Zuordnung über `dl_stream_id`
- `has_local_support()` gibt auf Apple `TRUE` zurück
- `ServerFileContentsResponse` wird in beiden Zweigen registriert
- `cliprdr_file_context_wait_for_files()` kommt hinzu (siehe Header)

Bekannte Einschränkung: Bei großen Dateien blockiert das Einfügen bis zum Ende
des Downloads (Grenze 10 Minuten). `NSFilePromiseProvider` wäre der spätere
Ausweg.

#### B) Prozentkodierung im Datei-Clipboard (Fehler von FreeRDP)

Dies ist die einzige Änderung im ganzen Patch, die **keine Anpassung für
RemoteDesk** ist, sondern ein Fehler von FreeRDP selbst. Er ist upstream
gemeldet als **[FreeRDP/FreeRDP#13130](https://github.com/FreeRDP/FreeRDP/issues/13130)**
und steht im aktuellen `master` unverändert.

`sdl_clip.cpp` erzeugt die URI-Liste mit `winpr_str_url_encode`, also
prozentkodiert. In `cliprdr_local_stream_update()` bekam anschließend
`append_entry()` den Namen und dekodierte ihn, während `is_directory()` und
`add_directory()` denselben Namen **kodiert** weiterverwendeten:

```
CreateFileW("…/Mein%20Ordner")  →  findet nichts
  →  gilt nicht als Verzeichnis  →  add_directory() läuft nie
  →  der gesamte Inhalt fehlt in der Liste
```

Zweiter, unabhängiger Fehler an derselben Stelle: `add_directory()` setzte
Kindpfade aus **kodiertem** Elternpfad und **rohem** Dateinamen zusammen
(`GetCombinedPath`). Ein `%` im Dateinamen wurde beim späteren Dekodieren als
Escape missdeutet.

Betroffen sind Leerzeichen, Umlaute, `%` und `#`. Schlichte Namen liefen
zufällig richtig — deshalb fiel der Fehler lange nicht auf.

**Lösung:** ein einziger Dekodierpunkt. `cliprdr_local_stream_update()`
dekodiert einmal, alles dahinter arbeitet mit echten Dateisystempfaden;
`cliprdr_local_file_new()` kopiert nur noch (`_strdup`) statt zu dekodieren.

Gemessen an denselben Testordnern:

| Ordner | ohne Fix | mit Fix |
|---|---|---|
| `Mein Ordner` | 1 Eintrag (Inhalt fehlt) | 5 ✓ |
| `Über` | 1 Eintrag (Inhalt fehlt) | 2 ✓ |
| `Schlicht` | 2 | 2 (unverändert) |

### `include/freerdp/client/client_cliprdr_file.h` (+16 Zeilen)

**Was:** Deklariert `cliprdr_file_context_wait_for_files(CliprdrFileContext*,
UINT32 timeoutMs)` samt Dokumentationskommentar.

**Warum:** Ohne FUSE holt der Client die Dateien wirklich ab, statt sie
einzuhängen. Vor dem Einfügen muss deshalb gewartet werden, sonst liegen
unvollständige Dateien vor. Der Kommentar hält ausdrücklich fest, dass die
Funktion nur aus dem Thread der Zwischenablage gerufen werden darf, nicht aus
dem Kanal-Thread.

---

## Teil B — SDL 3.2.28

FreeRDPs Bundling-Skript klont SDL selbst. Der Patch greift ausschließlich im
Cocoa-Treiber.

### `src/video/cocoa/SDL_cocoaclipboard.m` (206 Zeilen)

Vier Gründe, alle in derselben Datei:

**1. `text/uri-list` beim Lesen.** macOS legt kopierte Dateien als
`public.file-url` ab, einen Eintrag je Datei; FreeRDP erwartet wie alle
freedesktop-Programme `text/uri-list` mit allen Dateien in einer Liste.
`Cocoa_GetClipboardData` brach zudem beim ersten Treffer ab — eine
Mehrfachauswahl wäre ohnehin verloren gegangen. `GetMimeTypes` meldet den Typ
jetzt, sobald Datei-URLs vorliegen, `Cocoa_GetClipboardData` setzt ihn aus
allen Einträgen zusammen, `Cocoa_HasClipboardData` beantwortet ihn.

**2. File-Reference-URLs des Finders.** Der Finder legt keine Pfad-URLs ab,
sondern Kennungen der Form `file:///.file/id=6571367.121757495`. Wörtlich
gelesen ergibt das den Pfad `/.file/id=…`, den es nicht gibt — winpr meldete
`stat failed with Not a directory [20]` und lieferte eine leere Dateiliste.
`ResolveFileURL()` löst jede Datei-URL über `filePathURL` in den echten Pfad
auf; nicht auflösbare URLs bleiben unverändert, damit gewöhnliche Pfad-URLs
unberührt durchlaufen.

**3. `text/uri-list` beim Schreiben.** SDL übersetzt jeden MIME-Typ mit
`UTTypeCreatePreferredIdentifierForTag`. Für `text/uri-list` gibt es keinen
registrierten UTI, es entsteht ein dynamischer Typ (`dyn.…`). Den erkennt der
Finder nicht als Datei — das Menü „Einfügen" blieb aus, und der Client wurde nie
nach den Daten gefragt. `Cocoa_SetClipboardData` legt jetzt zusätzlich je Datei
ein Element mit `public.file-url` an. Ein eigener Datenanbieter liefert die URL
erst auf Nachfrage, also beim Einfügen; bloßes Kopieren im Gast löst keine
Übertragung aus.

**4. `Cocoa_CheckClipboardUpdate`** wird für den Abgleich aus
`SDL_cocoaevents.m` erreichbar gemacht.

### `src/video/cocoa/SDL_cocoaevents.m` (33 Zeilen)

**Was:** Zwei Eingriffe.

1. `Cocoa_PumpEvents` gleicht die Zwischenablage zusätzlich gedrosselt ab
   (höchstens alle 250 ms) über `Cocoa_CheckClipboardUpdate`.
2. `LoadMainMenuNibIfAvailable` blendet Fenster aus dem geladenen Nib mit
   `orderOut:` aus.

**Warum:**

Zu 1: macOS meldet Änderungen der Zwischenablage nicht als Ereignis. SDL glich
sie deshalb nur in `windowDidBecomeKey` ab — solange das RDP-Fenster den
Tastaturfokus behielt, bemerkte SDL nie, dass in einem anderen Programm etwas
kopiert wurde. Der Client meldete dem Server dauerhaft
`ClientFormatList: numFormats: 0`, Einfügen in der Sitzung blieb leer. Der
Abgleich kostet nur einen Vergleich von `[NSPasteboard changeCount]` und belastet
den Renderpfad nicht.

Zu 2: Enthält das Bundle ein Main-Nib, lädt SDL es für das Menü. Cocoa zeigt
dabei jedes Fenster an, das im Nib als „Visible At Launch" markiert ist. Der
SDL-Client liegt im Bundle des Cocoa-Clients und erbte dessen `MainMenu.nib` mit
einem Sitzungsfenster von 1024×768 — bei jeder Sitzung stand ein zweites, leeres
Fenster mit dem Titel „FreeRDP" daneben. Nachgemessen mit
`CGWindowListCopyWindowInfo`: ein Prozess, zwei Fenster. Das Menü bleibt
erhalten; da SDL seine Fenster ausschließlich über `SDL_CreateWindow` anlegt,
kann ein Fenster aus dem Nib nie ein SDL-Fenster sein.

### `src/video/cocoa/SDL_cocoawindow.m` (+95 Zeilen)

**Was:** Neu hinzu kommen die Klasse `RemoteDeskDragSource` (implementiert
`NSDraggingSource`) und die exportierte Funktion `RemoteDesk_StartFileDrag()`.

**Warum:** SDL kann Ablegen nur empfangen (`performDragOperation`), aber selbst
keinen Ziehvorgang beginnen — eine öffentliche Schnittstelle dafür gibt es
nicht. Der FreeRDP-Client holt die Dateien aus dem Gast über die Zwischenablage
ab und übergibt die fertigen Pfade hier.

Zwei Feinheiten, die nicht offensichtlich sind:

- Das Ereignis wird **frisch erzeugt**, statt `currentEvent` zu benutzen —
  zwischen dem Herausziehen und dem Ende des Downloads können Sekunden liegen,
  das zuletzt verarbeitete Ereignis taugt dann nicht mehr.
- Die Quelle liegt in einer **statischen Variablen**
  (`s_remotedesk_drag_source`), weil der Ziehvorgang sie nur schwach hält; ohne
  diese Referenz würde sie vorzeitig freigegeben.

Der Vorgang meldet `NSDragOperationCopy` (Kopieren, nicht Verschieben), da die
Dateien in einem temporären Ordner liegen.

---

## Übersicht

Umfang laut `git diff --stat` in beiden Quellbäumen:

| Datei | Umfang | Grund in einem Satz |
|---|---|---|
| **FreeRDP** | **+1145 / −35** | |
| `scripts/bundle-mac-os.sh` | +11 | Cocoa-Client mitbauen |
| `client/Mac/Keyboard.m` | 12 | Baufehler, der erst beim Bauen des Cocoa-Clients auffällt |
| `client/Mac/cli/MainMenu.xib` | 3 | Fenster war auf 1024×768 festgenagelt |
| `client/Mac/cli/AppDelegate.h` | +1 | Deklaration zu `mac_set_view_size` |
| `client/Mac/cli/AppDelegate.m` | 40 | Ansicht wuchs nicht mit dem Fenster mit |
| `client/SDL/SDL3/sdl_freerdp.cpp` | 18 | Ein Present je Paket → Renderlast 47 % |
| `client/SDL/SDL3/sdl_context.cpp` | 245 | Darstellung entkoppeln; Ablegen und Herausziehen |
| `client/SDL/SDL3/sdl_context.hpp` | 23 | Deklarationen dazu |
| `client/SDL/SDL3/sdl_clip.cpp` | 273 | Leere Formatliste, Dateien beidseitig, Ziehen |
| `client/SDL/SDL3/sdl_clip.hpp` | 31 | Deklarationen dazu |
| `client/common/client_cliprdr_file.c` | 507 | Download ohne FUSE **+ Fehler #13130** |
| `include/freerdp/client/client_cliprdr_file.h` | +16 | `cliprdr_file_context_wait_for_files` |
| **SDL** | **+332 / −2** | |
| `src/video/cocoa/SDL_cocoaclipboard.m` | 206 | `text/uri-list` beidseitig, Finder-URLs auflösen |
| `src/video/cocoa/SDL_cocoaevents.m` | 33 | Zwischenablage abgleichen; Geisterfenster ausblenden |
| `src/video/cocoa/SDL_cocoawindow.m` | +95 | Ziehvorgang starten (SDL kann das nicht) |

## Einordnung

Der Unterschied ist für spätere Leser wesentlich:

**Ein Fehler von FreeRDP** ist nur der Abschnitt B in `client_cliprdr_file.c`
(Prozentkodierung). Er betrifft jede Plattform, ist upstream als
[#13130](https://github.com/FreeRDP/FreeRDP/issues/13130) gemeldet und sollte
entfallen, sobald er dort behoben ist.

**Ein Baufehler** ist `Keyboard.m` — er fällt upstream nicht auf, weil der
Cocoa-Client dort nicht gebaut wird.

**Alles Übrige sind Anpassungen für RemoteDesk**: Funktionen, die FreeRDP und
SDL auf macOS bewusst nicht anbieten (Dateien ohne FUSE, Ziehen und
Fallenlassen), oder Verhalten, das für diesen Anwendungsfall nicht taugt
(Fenstergröße, Renderpfad, Abgleich der Zwischenablage). Sie sind kein
Kandidat für einen Upstream-Beitrag in dieser Form.

## Pflege

Die **Patchdateien sind die maßgebliche Quelle**, nicht die Arbeitskopien in
`.build/`. Jeder Bau setzt die betroffenen Dateien per `git checkout --` zurück
und wendet den Patch neu an — unaufbewahrte Änderungen im Quellbaum gehen dabei
verloren.

Patch nach einer Änderung neu erzeugen (Prosakopf erhalten, Diff neu):

```bash
cd remote-client/.build/freerdp/FreeRDP
head -n 136 ../../../scripts/patches/freerdp-3.26.0-macos-client.patch > /tmp/head.txt
git diff -- $(git diff --name-only) > /tmp/diff.patch
cat /tmp/head.txt /tmp/diff.patch \
  > ../../../scripts/patches/freerdp-3.26.0-macos-client.patch
```

Vor dem Bau immer `git apply --check` laufen lassen — ein Patch, der nicht mehr
passt, würde sonst stillschweigend einen ungepatchten Bau erzeugen.
