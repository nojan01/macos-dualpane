import { Show, For, createSignal } from "solid-js";
import { t, getResolvedLang } from "../i18n";

const [open, setOpen] = createSignal(false);
const [sectionIdx, setSectionIdx] = createSignal(0);

/** Öffnet den Hilfe-Dialog (beginnt immer bei „Erste Schritte"). */
export async function openHelp() {
  setSectionIdx(0);
  setOpen(true);
}

interface Item {
  term: string;
  desc: string;
}
interface Group {
  title: string;
  items: Item[];
}
interface Section {
  title: string;
  intro?: string;
  items?: Item[];
  groups?: Group[];
}

const CONTENT: Record<
  "de" | "en",
  { title: string; navLabel: string; sections: Section[] }
> = {
  de: {
    title: "DualBeam – Hilfe",
    navLabel: "Hilfethemen",
    sections: [
      {
        title: "Erste Schritte",
        intro:
          "Willkommen bei DualBeam! Diese Seite erklärt die Grundidee – die Kapitel links vertiefen die einzelnen Themen.",
        items: [
          {
            term: "Das Zwei-Fenster-Prinzip",
            desc: "DualBeam zeigt zwei Dateibereiche (Panes) nebeneinander. Der Clou: Kopieren, Verschieben und Synchronisieren wirken immer vom aktiven Bereich in den gegenüberliegenden – ein Ziel muss nie extra ausgewählt werden.",
          },
          {
            term: "Aktiver Bereich",
            desc: "Der Bereich mit dem Tastaturfokus ist farblich hervorgehoben. Mit Tab oder einem Klick wechselst du die Seite.",
          },
          {
            term: "Ordner öffnen",
            desc: "Doppelklick oder Enter öffnet einen Ordner, ⌫ (Rücktaste) oder ⌘↑ geht eine Ebene nach oben. Die Pfadleiste zeigt den aktuellen Ort – ein Klick auf ein Segment springt direkt dorthin.",
          },
          {
            term: "Die erste Kopie",
            desc: "Datei mit Klick oder Pfeiltasten markieren, dann F5 drücken – sie landet im anderen Bereich. F6 verschiebt stattdessen. Alternativ einfach mit der Maus hinüberziehen.",
          },
          {
            term: "Tippen zum Springen",
            desc: "Einfach lostippen: Die Auswahl springt zum nächsten Eintrag, der mit den getippten Buchstaben beginnt.",
          },
          {
            term: "Hilfe unterwegs",
            desc: "F1 blendet eine Kurzreferenz der wichtigsten Tasten in der Statusleiste ein. Dieses Fenster öffnest du jederzeit über das ?-Symbol in der Werkzeugleiste.",
          },
        ],
      },
      {
        title: "Oberfläche im Überblick",
        intro: "Die wichtigsten Bereiche des Fensters von oben nach unten:",
        items: [
          {
            term: "Werkzeugleiste",
            desc: "Oben: Seitenleiste, Vorschau, Ansichtsmodi (Spalten, Vergleich, Folgemodus), Drag-Verhalten, versteckte Dateien, Aktualisieren, Rückgängig, Terminal, Design, Sprache und Hilfe. Jeder Knopf erklärt sich beim Überfahren mit der Maus.",
          },
          {
            term: "Pfadleiste",
            desc: "Zeigt den Pfad des Bereichs. Ein Klick auf ein Segment wechselt dorthin; die Pfeile daneben gehen zum vorherigen Ordner bzw. eine Ebene nach oben.",
          },
          {
            term: "Tabs",
            desc: "Jeder Bereich kann mehrere Ordner in Tabs halten: ⌘T öffnet einen neuen Tab, ⌘W schließt ihn, ⌘1–9 wechselt direkt.",
          },
          {
            term: "Seitenleiste (⌘B)",
            desc: "Enthält Favoriten (eigene Schnellzugriffe – über „+“ den aktuellen Ordner hinzufügen, mit ⌥1–⌥9 öffnen), eingebundene Volumes, Netzwerk-Server, gespeicherte Sync-Profile und – falls RemoteDeskRDP installiert ist – die dort eingerichteten RDP-Verbindungen.",
          },
          {
            term: "Dateiliste",
            desc: "Klick markiert, ⌘-Klick ergänzt einzelne Einträge, ⇧-Klick markiert einen Bereich. In der erweiterten Ansicht sortiert ein Klick auf den Spaltenkopf.",
          },
          {
            term: "Funktionsleiste",
            desc: "Unten: die wichtigsten Aktionen als Knöpfe mit ihren F-Tasten – ideal, um die Kürzel nebenbei zu lernen.",
          },
          {
            term: "Statusleiste",
            desc: "Ganz unten: Anzahl und Größe der markierten Einträge sowie freier Speicherplatz. F1 blendet hier die Tasten-Kurzhilfe ein.",
          },
        ],
      },
      {
        title: "Dateien verwalten",
        items: [
          {
            term: "Kopieren & Verschieben (F5/F6)",
            desc: "Wirkt auf die Auswahl und zielt immer auf den anderen Bereich. Läuft als Auftrag mit Fortschrittsbalken; laufende Übertragungen lassen sich abbrechen.",
          },
          {
            term: "Drag & Drop",
            desc: "Einträge lassen sich zwischen den Bereichen und in Ordner ziehen. Die Standardaktion (Kopieren oder Verschieben) stellst du in der Werkzeugleiste um; ⌥ beim Ziehen nutzt jeweils die andere Aktion.",
          },
          {
            term: "Zwischenablage (⌘C/⌘V)",
            desc: "Kopiert markierte Dateien in die Zwischenablage und fügt sie ein – das funktioniert auch zwischen DualBeam und dem Finder.",
          },
          {
            term: "Löschen (F8/Entf/⌘⌫)",
            desc: "Legt die Auswahl in den Papierkorb; mit ⇧ wird ohne Nachfrage endgültig gelöscht. ⌘Z macht die letzte Löschung rückgängig. Achtung: Netzlaufwerke haben oft keinen Papierkorb – dort ist Löschen endgültig (DualBeam warnt vorher).",
          },
          {
            term: "Umbenennen (F2/⌘R)",
            desc: "F2 benennt direkt in der Liste um. ⌘R öffnet das Mehrfach-Umbenennen mit Mustern, um viele Dateien auf einmal umzubenennen.",
          },
          {
            term: "Duplizieren (⌘D)",
            desc: "Erzeugt eine Kopie des Eintrags im selben Ordner.",
          },
          {
            term: "Neuer Ordner (F7)",
            desc: "Legt im aktiven Bereich einen neuen Ordner an.",
          },
          {
            term: "ZIP (⌘E)",
            desc: "Packt die Auswahl in ein ZIP-Archiv bzw. entpackt ein markiertes Archiv.",
          },
          {
            term: "Filtern (⌘F)",
            desc: "Filtert die aktuelle Liste nach Namen. Esc setzt den Filter zurück.",
          },
          {
            term: "Schnellansicht (Leertaste/F3)",
            desc: "Öffnet die macOS-Schnellansicht (Quick Look) für den markierten Eintrag.",
          },
          {
            term: "Eigenschaften (⌥⌘I)",
            desc: "Zeigt Details wie Größe, Berechtigungen und Änderungsdatum.",
          },
        ],
      },
      {
        title: "Ansichten & Modi",
        intro:
          "Die Modi werden über die Symbole in der Werkzeugleiste ein- und ausgeschaltet.",
        items: [
          {
            term: "Erweiterte Ansicht (Spalten)",
            desc: "Zeigt Größe und Änderungsdatum in Spalten. Ein Klick auf den Spaltenkopf sortiert, ein weiterer Klick kehrt die Richtung um.",
          },
          {
            term: "Vergleichs-Modus",
            desc: "Färbt Einträge im Vergleich zum anderen Bereich ein: nur hier vorhanden, unterschiedlich oder identisch (nach Größe und Änderungszeit). Praktisch, um zwei Ordner vor einem Sync zu prüfen.",
          },
          {
            term: "Folgemodus",
            desc: "Wählst du im aktiven Bereich einen Ordner an (Klick oder Pfeiltasten), öffnet der andere Bereich automatisch dessen Inhalt. So blätterst du einen Verzeichnisbaum bequem zweispaltig durch. Programme (.app) werden dabei nicht betreten.",
          },
          {
            term: "Vorschau-Bereich (⌘I)",
            desc: "Blendet eine Vorschau der markierten Datei ein (Bilder, Texte u. a.).",
          },
          {
            term: "Versteckte Dateien (⌘.)",
            desc: "Blendet Dateien mit führendem Punkt ein oder aus.",
          },
          {
            term: "Design & Sprache",
            desc: "Über die Werkzeugleiste: Design System/Hell/Dunkel und Sprache Auto/Deutsch/Englisch.",
          },
        ],
      },
      {
        title: "Tastenkombinationen",
        groups: [
          {
            title: "Navigation",
            items: [
              { term: "↑ / ↓", desc: "Auswahl bewegen" },
              { term: "⇞ / ⇟", desc: "20 Einträge springen" },
              { term: "Home / End", desc: "An den Anfang / das Ende" },
              { term: "Enter", desc: "Ordner öffnen bzw. Datei starten" },
              { term: "⌫ oder ⌘↑", desc: "Eine Ebene nach oben" },
              { term: "Tab", desc: "Aktiven Bereich wechseln" },
              {
                term: "Buchstaben",
                desc: "Type-ahead: zum passenden Eintrag springen",
              },
              { term: "Esc", desc: "Filter zurücksetzen" },
            ],
          },
          {
            title: "Dateien",
            items: [
              { term: "F2", desc: "Umbenennen" },
              { term: "⌘R", desc: "Mehrfach-Umbenennen" },
              { term: "F5", desc: "Kopieren in den anderen Bereich" },
              { term: "F6", desc: "Verschieben in den anderen Bereich" },
              { term: "F7", desc: "Neuer Ordner" },
              {
                term: "F8 / Entf / ⌘⌫",
                desc: "Löschen – mit ⇧ endgültig, ohne Nachfrage",
              },
              { term: "⌘D", desc: "Duplizieren" },
              { term: "⌘C / ⌘V", desc: "Zwischenablage: kopieren / einfügen" },
              { term: "⌘E", desc: "ZIP packen bzw. entpacken" },
              { term: "⌘Z", desc: "Letzte Löschung rückgängig" },
              {
                term: "Leertaste / F3",
                desc: "Schnellansicht (Quick Look)",
              },
              { term: "⌥⌘I", desc: "Eigenschaften anzeigen" },
            ],
          },
          {
            title: "Ansicht & Fenster",
            items: [
              { term: "F1", desc: "Tasten-Kurzhilfe in der Statusleiste" },
              { term: "⌘I", desc: "Vorschau-Bereich ein/aus" },
              { term: "⌘.", desc: "Versteckte Dateien ein/aus" },
              { term: "⌘⇧R", desc: "Ordner neu einlesen" },
              { term: "⌘B", desc: "Seitenleiste ein/aus" },
              { term: "⌘F", desc: "Filter / Suche" },
              { term: "⌘K", desc: "Mit Server verbinden" },
              { term: "⌘N", desc: "Neues Fenster" },
              { term: "⌘T / ⌘W", desc: "Tab öffnen / schließen" },
              { term: "⌘1–9", desc: "Tab wechseln" },
              { term: "⌥1–⌥9", desc: "Favorit öffnen" },
            ],
          },
        ],
      },
      {
        title: "Verzeichnisse synchronisieren",
        intro:
          "Wähle einen Ordner und nutze im Kontextmenü „Synchronisieren → …“. DualBeam erstellt zuerst immer eine Vorschau; erst danach werden die passenden Dateien kopiert oder gelöscht. Bei langsamen Netzlaufwerken läuft die Vorschau im Hintergrund – die App bleibt dabei bedienbar.",
        items: [
          {
            term: "Einweg-Synchronisation",
            desc: "Die Quelle ist maßgeblich. Neue und geänderte Dateien werden ins Ziel kopiert. Überzählige Dateien im Ziel erscheinen in der Vorschau und werden nur gelöscht, wenn die Option ausdrücklich aktiviert wird.",
          },
          {
            term: "Zwei-Wege-Synchronisation",
            desc: "Erkennt Änderungen in beide Richtungen. Gleichzeitig oder nicht eindeutig geänderte Dateien erscheinen als Konflikt; wähle dafür die linke Version, die rechte Version oder „Überspringen“. Automatisches Löschen ist in diesem Modus ausgeschaltet.",
          },
          {
            term: "Übertragungsart (HiDrive)",
            desc: "Liegt das Ziel auf dem eingebundenen HiDrive-WebDAV-Laufwerk, lässt sich zwischen dem direkten Kopieren über das Laufwerk und rsync über SSH wählen. rsync überträgt nur Änderungen und ist bei vielen Dateien deutlich schneller; es arbeitet stets einweg. Bei lokalen Zielen erscheint keine Auswahl.",
          },
          {
            term: "S3- und Swift-Ziele",
            desc: "Ein eingehängter S3-Bucket oder Swift-Container kann wie ein WebDAV-Laufwerk als Quelle oder Ziel eines Sync-Profils verwendet werden. Die Vorschau liest Verzeichnisse auf dem Objekt-Speicher; bei der ersten Abfrage großer Container kann das trotz Ausschlüssen etwas dauern. Ausgeschlossene Teilbäume werden jedoch nicht weiter betreten.",
          },
          {
            term: "Ausschlussregeln",
            desc: "Unter „Ausschlussregeln“ gilt eine Regel pro Zeile für relative Pfade auf beiden Seiten. Namen, Pfade sowie * und ? werden unterstützt. Ausgeschlossene Ordner und ihre Inhalte erscheinen nicht in der Vorschau und werden nicht rekursiv eingelesen. Eine Datei .dualbeamignore im Quellordner wird zusätzlich berücksichtigt.",
          },
          {
            term: "SHA-256-Inhaltsprüfung",
            desc: "Prüft gleich große Dateien zusätzlich anhand ihres vollständigen Inhalts. Das ist sehr langsam, weil Quelle und Ziel komplett gelesen werden – besonders bei großen Ordnern oder Netzlaufwerken. Nur einschalten, wenn Größen- und Zeitstempelvergleich nicht genügt.",
          },
          {
            term: "Sync-Profile",
            desc: "Ein Profil speichert Quelle, Ziel und Optionen. Es kann im Synchronisationsdialog ausgewählt, aktualisiert oder gelöscht werden. In der Seitenleiste unter „Sync-Profile“ startet ein Klick das gespeicherte Profil unabhängig von den aktuell geöffneten Ordnern. Nicht auflösbare Zwei-Wege-Konflikte werden dabei sicher übersprungen.",
          },
          {
            term: "Remote-Desktop",
            desc: "Ist RemoteDeskRDP installiert und dort mindestens eine Verbindung eingerichtet, erscheint in der Seitenleiste der Abschnitt „Remote-Desktop“. Ein Klick öffnet die Sitzung direkt in RemoteDeskRDP – DualBeam selbst baut keine RDP-Verbindung auf und speichert keine Kennwörter. Gelesen werden nur Name und Adresse; die Kennwörter bleiben im Schlüsselbund von RemoteDeskRDP. Neu angelegte Verbindungen erscheinen, sobald das DualBeam-Fenster wieder in den Vordergrund kommt. Fehlt der Abschnitt, ist die App nicht installiert oder es ist dort noch keine Verbindung eingerichtet.",
          },
        ],
      },
      {
        title: "Netzwerkprotokolle",
        intro:
          "Über „Mit Server verbinden“ (⌘K) oder + im Bereich „Netzwerk“ bindest du Freigaben und Objekt-Speicher ein. Unterstützte Protokolle:",
        items: [
          {
            term: "smb://",
            desc: "Windows-/Samba-Freigaben. Beispiel: smb://server/freigabe. Das Standardprotokoll für die meisten NAS-Geräte und Windows-Rechner.",
          },
          {
            term: "https:// (WebDAV)",
            desc: "Verschlüsselte WebDAV-Freigaben (z. B. Nextcloud, HiDrive, ownCloud). Beispiel: https://server/webdav.",
          },
          {
            term: "sftp:// (SSH)",
            desc: "Dateizugriff über SSH – bei vielen Hostern die übliche Wahl. Beispiel: sftp://sftp.example.com/daten. Es öffnet sich ein eigener Dialog für Benutzername und Passwort. Beim ersten Verbinden zeigt DualBeam den Fingerabdruck des Servers zur Bestätigung; danach merkt es ihn sich.",
          },
          {
            term: "ftps:// und ftpes://",
            desc: "FTP über TLS. ftps:// ist die implizite Variante (Port 990), ftpes:// die explizite mit AUTH TLS (Port 21). Im Dialog lässt sich zwischen beiden umschalten.",
          },
          {
            term: "S3 und OpenStack Swift",
            desc: "Im Dialog „Netzlaufwerk hinzufügen“ wählst du S3 oder OpenStack Swift und hinterlegst Endpunkt, Anmeldedaten sowie optional Bucket bzw. Container. Für Swift sind Keystone v2 und v3 auswählbar. Access Key, Secret Access Key und Swift-Kennwort werden ausschließlich im macOS-Schlüsselbund gespeichert. Ein verbundenes Ziel verhält sich anschließend wie ein Netzwerkordner und lässt sich auch synchronisieren.",
          },
          {
            term: "Netzwerk in der Seitenleiste",
            desc: "Gespeicherte Server sowie S3-/Swift-Profile erscheinen im Bereich „Netzwerk“. Ein verbundenes Lesezeichen öffnet den Mountpunkt; ein getrenntes verbindet sich per Klick. ⚙ bearbeitet die Einstellungen, ↻ verbindet neu, ⏏ hängt nur aus und behält das Profil, × entfernt es dauerhaft. Nicht als Lesezeichen gespeicherte Netzwerk-Volumes erscheinen ebenfalls dort.",
          },
          {
            term: "Löschen auf Netzlaufwerken",
            desc: "Nicht jedes Netzlaufwerk stellt einen Papierkorb bereit; DualBeam weist darauf hin, das Löschen kann dann endgültig sein. Große Ordner werden bei WebDAV direkt als Collection gelöscht, bei S3, Swift, SFTP und FTPS direkt über rclone statt Datei für Datei über den Mount. Danach wird die betroffene Ordneransicht automatisch aktualisiert. SMB verwendet den nativen macOS-Löschweg.",
          },
          {
            term: "Unsichere lokale Protokolle",
            desc: "http, ftp, afp, nfs und cifs sind ausschließlich für direkte private, Link-Local- oder Loopback-IP-Adressen verfügbar und erfordern eine ausdrückliche Warnbestätigung.",
          },
          {
            term: "rclone im Hintergrund",
            desc: "macOS bringt SFTP, FTP/FTPS, S3 und OpenStack Swift nicht selbst als beschreibbare Laufwerke mit. DualBeam liefert dafür rclone mit und bindet diese Ziele über den NFS-Dienst von macOS ein. Ein Administratorpasswort ist nicht nötig. Die Verbindung endet automatisch, wenn du DualBeam beendest.",
          },
        ],
      },
      {
        title: "Anmeldedaten & Schlüsselbund",
        intro:
          "WebDAV- und SMB-Verbindungen übernimmt macOS (Finder); S3, Swift, SFTP und FTPS richtet DualBeam mit rclone ein. Daraus ergeben sich einige Eigenheiten:",
        items: [
          {
            term: "S3-/Swift-Geheimnisse",
            desc: "Das Objekt-Speicherprofil enthält nur die nicht geheimen Verbindungsdaten. Secret Access Key bzw. Swift-Kennwort liegen getrennt im macOS-Schlüsselbund. Beim Bearbeiten darf das Feld leer bleiben, um das gespeicherte Geheimnis zu behalten.",
          },
          {
            term: "Passwortabfrage",
            desc: "Hast du beim ersten Verbinden „Passwort sichern“ aktiviert, liegt es im macOS-Schlüsselbund. macOS mountet dann ohne erneute Passwortabfrage – das ist normal und kein Fehler von DualBeam.",
          },
          {
            term: "Dialog „webdavfs_agent…“",
            desc: "Dieser Schlüsselbund-Dialog stammt von macOS. Er fragt nicht nach deinem HiDrive-Passwort, sondern ob der System-Dienst das gespeicherte Passwort lesen darf.",
          },
          {
            term: "Mountet trotz „Nicht erlauben“",
            desc: "Wenn der Schlüsselbund-Eintrag „immer erlauben“ für webdavfs_agent gesetzt ist, greift macOS direkt zu und hängt das Volume parallel ein. Das ist macOS-Verhalten, kein DualBeam-Bug.",
          },
          {
            term: "Wieder nachfragen lassen",
            desc: "In der Schlüsselbundverwaltung den Eintrag des Servers öffnen → Reiter „Zugriff“ → webdavfs_agent entfernen bzw. auf „nachfragen“ stellen. Oder den gespeicherten Eintrag löschen, damit macOS beim nächsten Mount neu nach dem Passwort fragt.",
          },
          {
            term: "Nachfragen abschalten",
            desc: "Den ständigen Dialog stoppst du in der Schlüsselbundverwaltung: Eintrag des Servers doppelklicken → Reiter „Zugriff“ → entweder „Allen Programmen Zugriff erlauben“ wählen oder sicherstellen, dass webdavfs_agent in der Liste steht und auf „immer erlauben“ steht → „Änderungen sichern“. Kommt der Dialog trotzdem, gibt es oft einen doppelten/alten Eintrag für denselben Host – diesen löschen.",
          },
          {
            term: "NFS",
            desc: "NFS kennt keine Passwortabfrage – der Zugriff wird über die Export-/Freigabeliste des Servers (Host/IP) geregelt.",
          },
        ],
      },
      {
        title: "Tipps & Problemlösungen",
        items: [
          {
            term: "Langsames Netzlaufwerk?",
            desc: "Verzeichnislisten und Sync-Vorschauen auf Netzlaufwerken laufen im Hintergrund – die App bleibt bedienbar. Große Ordner brauchen beim ersten Einlesen dennoch Zeit.",
          },
          {
            term: "Dauerhafte Ausschlüsse",
            desc: "Eine Datei .dualbeamignore im Quellordner (eine Regel pro Zeile, * und ? erlaubt) wird bei jeder Synchronisation zusätzlich berücksichtigt.",
          },
          {
            term: "Gelöscht statt Papierkorb?",
            desc: "Netzlaufwerke stellen meist keinen Papierkorb bereit; dort ist Löschen endgültig. DualBeam weist vorher darauf hin.",
          },
          {
            term: "Favoriten pflegen",
            desc: "In der Seitenleiste den aktuellen Ordner über „+“ hinzufügen; per Kontextmenü umbenennen oder entfernen. ⌥1–⌥9 öffnet die ersten neun Favoriten.",
          },
          {
            term: "Etwas ging schief?",
            desc: "⌘Z stellt die letzte Löschung wieder her. Laufende Aufträge lassen sich über die Fortschrittsanzeige abbrechen.",
          },
          {
            term: "Tastenkürzel lernen",
            desc: "Die Funktionsleiste unten zeigt die wichtigsten F-Tasten; F1 blendet zusätzlich eine Kurzreferenz in der Statusleiste ein.",
          },
        ],
      },
    ],
  },
  en: {
    title: "DualBeam – Help",
    navLabel: "Help topics",
    sections: [
      {
        title: "Getting started",
        intro:
          "Welcome to DualBeam! This page explains the core idea – the chapters on the left cover each topic in depth.",
        items: [
          {
            term: "The dual-pane principle",
            desc: "DualBeam shows two file panes side by side. The trick: copy, move and sync always act from the active pane onto the opposite one – you never have to pick a destination.",
          },
          {
            term: "Active pane",
            desc: "The pane with keyboard focus is highlighted. Press Tab or click to switch sides.",
          },
          {
            term: "Opening folders",
            desc: "Double-click or Enter opens a folder, ⌫ (Backspace) or ⌘↑ goes up one level. The path bar shows your location – click a segment to jump there directly.",
          },
          {
            term: "Your first copy",
            desc: "Select a file by clicking or with the arrow keys, then press F5 – it lands in the other pane. F6 moves instead. Or simply drag it across with the mouse.",
          },
          {
            term: "Type to jump",
            desc: "Just start typing: the selection jumps to the next entry starting with the typed letters.",
          },
          {
            term: "Help along the way",
            desc: "F1 shows a quick key reference in the status bar. You can reopen this window any time via the ? icon in the toolbar.",
          },
        ],
      },
      {
        title: "The interface",
        intro: "The main areas of the window, top to bottom:",
        items: [
          {
            term: "Toolbar",
            desc: "At the top: sidebar, preview, view modes (columns, compare, follow), drag behaviour, hidden files, refresh, undo, Terminal, theme, language and help. Hover any button for an explanation.",
          },
          {
            term: "Path bar",
            desc: "Shows the pane's path. Click a segment to go there; the arrows next to it go to the previous folder or up one level.",
          },
          {
            term: "Tabs",
            desc: "Each pane can hold several folders in tabs: ⌘T opens a new tab, ⌘W closes it, ⌘1–9 switches directly.",
          },
          {
            term: "Sidebar (⌘B)",
            desc: "Contains favorites (your own shortcuts – add the current folder via “+”, open with ⌥1–⌥9), mounted volumes, network servers, saved sync profiles and – if RemoteDeskRDP is installed – the RDP connections configured there.",
          },
          {
            term: "File list",
            desc: "Click selects, ⌘-click adds single entries, ⇧-click selects a range. In the extended view, clicking a column header sorts the list.",
          },
          {
            term: "Function key bar",
            desc: "At the bottom: the most important actions as buttons with their F-keys – a great way to learn the shortcuts.",
          },
          {
            term: "Status bar",
            desc: "At the very bottom: number and size of the selected entries plus free disk space. F1 shows the quick key reference here.",
          },
        ],
      },
      {
        title: "Working with files",
        items: [
          {
            term: "Copy & move (F5/F6)",
            desc: "Acts on the selection and always targets the other pane. Runs as a job with a progress bar; running transfers can be cancelled.",
          },
          {
            term: "Drag & drop",
            desc: "Entries can be dragged between panes and into folders. Set the default action (copy or move) in the toolbar; holding ⌥ while dragging uses the other action.",
          },
          {
            term: "Clipboard (⌘C/⌘V)",
            desc: "Copies selected files to the clipboard and pastes them – this also works between DualBeam and the Finder.",
          },
          {
            term: "Delete (F8/Del/⌘⌫)",
            desc: "Moves the selection to the Trash; with ⇧ it is deleted permanently without asking. ⌘Z undoes the last deletion. Note: network shares often have no Trash – deleting there is permanent (DualBeam warns first).",
          },
          {
            term: "Rename (F2/⌘R)",
            desc: "F2 renames inline in the list. ⌘R opens batch rename with patterns to rename many files at once.",
          },
          {
            term: "Duplicate (⌘D)",
            desc: "Creates a copy of the entry in the same folder.",
          },
          {
            term: "New folder (F7)",
            desc: "Creates a folder in the active pane.",
          },
          {
            term: "ZIP (⌘E)",
            desc: "Packs the selection into a ZIP archive or extracts a selected archive.",
          },
          {
            term: "Filter (⌘F)",
            desc: "Filters the current list by name. Esc clears the filter.",
          },
          {
            term: "Quick Look (Space/F3)",
            desc: "Opens the macOS Quick Look preview for the selected entry.",
          },
          {
            term: "Properties (⌥⌘I)",
            desc: "Shows details such as size, permissions and modification date.",
          },
        ],
      },
      {
        title: "Views & modes",
        intro: "The modes are toggled via the icons in the toolbar.",
        items: [
          {
            term: "Extended view (columns)",
            desc: "Shows size and modification date in columns. Click a column header to sort; click again to reverse the order.",
          },
          {
            term: "Compare mode",
            desc: "Colours entries compared to the other pane: only here, different, or identical (by size and modification time). Handy for checking two folders before a sync.",
          },
          {
            term: "Follow mode",
            desc: "When you select a folder in the active pane (click or arrow keys), the other pane automatically opens its contents. Great for browsing a directory tree in two columns. Applications (.app) are not entered.",
          },
          {
            term: "Preview pane (⌘I)",
            desc: "Shows a preview of the selected file (images, text and more).",
          },
          {
            term: "Hidden files (⌘.)",
            desc: "Shows or hides files starting with a dot.",
          },
          {
            term: "Theme & language",
            desc: "Via the toolbar: theme System/Light/Dark and language Auto/German/English.",
          },
        ],
      },
      {
        title: "Keyboard shortcuts",
        groups: [
          {
            title: "Navigation",
            items: [
              { term: "↑ / ↓", desc: "Move the selection" },
              { term: "⇞ / ⇟", desc: "Jump 20 entries" },
              { term: "Home / End", desc: "Go to the beginning / end" },
              { term: "Enter", desc: "Open folder or launch file" },
              { term: "⌫ or ⌘↑", desc: "Go up one level" },
              { term: "Tab", desc: "Switch active pane" },
              {
                term: "Letters",
                desc: "Type-ahead: jump to the matching entry",
              },
              { term: "Esc", desc: "Clear the filter" },
            ],
          },
          {
            title: "Files",
            items: [
              { term: "F2", desc: "Rename" },
              { term: "⌘R", desc: "Batch rename" },
              { term: "F5", desc: "Copy to the other pane" },
              { term: "F6", desc: "Move to the other pane" },
              { term: "F7", desc: "New folder" },
              {
                term: "F8 / Del / ⌘⌫",
                desc: "Delete – with ⇧ permanently, without asking",
              },
              { term: "⌘D", desc: "Duplicate" },
              { term: "⌘C / ⌘V", desc: "Clipboard: copy / paste" },
              { term: "⌘E", desc: "Pack / extract ZIP" },
              { term: "⌘Z", desc: "Undo last deletion" },
              { term: "Space / F3", desc: "Quick Look" },
              { term: "⌥⌘I", desc: "Show properties" },
            ],
          },
          {
            title: "View & window",
            items: [
              { term: "F1", desc: "Quick key reference in the status bar" },
              { term: "⌘I", desc: "Toggle preview pane" },
              { term: "⌘.", desc: "Toggle hidden files" },
              { term: "⌘⇧R", desc: "Reload folder" },
              { term: "⌘B", desc: "Toggle sidebar" },
              { term: "⌘F", desc: "Filter / search" },
              { term: "⌘K", desc: "Connect to server" },
              { term: "⌘N", desc: "New window" },
              { term: "⌘T / ⌘W", desc: "Open / close tab" },
              { term: "⌘1–9", desc: "Switch tab" },
              { term: "⌥1–⌥9", desc: "Open favorite" },
            ],
          },
        ],
      },
      {
        title: "Folder synchronization",
        intro:
          "Select a folder and use “Sync → …” in its context menu. DualBeam always builds a preview first; only then are the appropriate files copied or deleted. On slow network shares, the preview runs in the background so the app remains usable.",
        items: [
          {
            term: "One-way synchronization",
            desc: "The source is authoritative. New and changed files are copied to the target. Extra target files are listed in the preview and are deleted only when that option is explicitly enabled.",
          },
          {
            term: "Two-way synchronization",
            desc: "Detects changes in both directions. Files changed at the same time or without an unambiguous direction are shown as conflicts; choose the left version, right version, or “Skip”. Automatic deletion is disabled in this mode.",
          },
          {
            term: "Transfer method (HiDrive)",
            desc: "If the target is on the mounted HiDrive WebDAV drive, you can choose between copying directly via the drive and rsync over SSH. rsync transfers only changes and is much faster with many files; it always works one-way. For local targets no choice is shown.",
          },
          {
            term: "S3 and Swift targets",
            desc: "A mounted S3 bucket or Swift container can be used as the source or destination of a sync profile just like a WebDAV drive. The preview reads directories on the object store, so its first scan of a large container can take time even with exclusions. Excluded subtrees are not entered further, however.",
          },
          {
            term: "Exclusion rules",
            desc: "Under “Exclusion rules”, enter one rule per line for relative paths on both sides. Names, paths, * and ? are supported. Excluded folders and their contents do not appear in the preview and are not read recursively. A .dualbeamignore file in the source folder is also applied.",
          },
          {
            term: "SHA-256 content verification",
            desc: "Also checks same-size files using their complete contents. This is very slow because both source and target are read in full — especially for large folders or network shares. Use it only when size and modification time are not sufficient.",
          },
          {
            term: "Sync profiles",
            desc: "A profile stores source, target and options. It can be selected, updated or deleted in the synchronization dialog. Clicking a profile in the sidebar’s “Sync profiles” section starts it regardless of the folders currently open in the panes. Unresolved two-way conflicts are skipped safely.",
          },
          {
            term: "Remote desktop",
            desc: "If RemoteDeskRDP is installed and has at least one connection configured, the sidebar shows a “Remote desktop” section. Clicking an entry opens the session in RemoteDeskRDP – DualBeam never speaks RDP itself and stores no passwords. Only the name and address are read; passwords stay in RemoteDeskRDP’s keychain. Newly created connections appear as soon as the DualBeam window comes back to the front. If the section is missing, the app is not installed or no connection has been configured there yet.",
          },
        ],
      },
      {
        title: "Network protocols",
        intro:
          "Use “Connect to server” (⌘K) or + in the “Network” section to mount shares and object storage. Supported protocols:",
        items: [
          {
            term: "smb://",
            desc: "Windows/Samba shares. Example: smb://server/share. The default protocol for most NAS devices and Windows machines.",
          },
          {
            term: "https:// (WebDAV)",
            desc: "Encrypted WebDAV shares (e.g. Nextcloud, HiDrive, ownCloud). Example: https://server/webdav.",
          },
          {
            term: "sftp:// (SSH)",
            desc: "File access over SSH — the usual choice at many hosting providers. Example: sftp://sftp.example.com/data. A dedicated dialog asks for user name and password. On the first connection DualBeam shows the server's fingerprint for confirmation and remembers it afterwards.",
          },
          {
            term: "ftps:// and ftpes://",
            desc: "FTP over TLS. ftps:// is the implicit variant (port 990), ftpes:// the explicit one using AUTH TLS (port 21). The dialog lets you switch between them.",
          },
          {
            term: "S3 and OpenStack Swift",
            desc: "In “Add network drive”, choose S3 or OpenStack Swift and enter the endpoint, credentials, and optionally a bucket or container. Keystone v2 and v3 are available for Swift. Access key, secret access key, and Swift password are stored only in the macOS Keychain. Once connected, the target behaves like a network folder and can be synchronized.",
          },
          {
            term: "Network in the sidebar",
            desc: "Saved servers and S3/Swift profiles appear in the “Network” section. Clicking a connected bookmark opens its mount point; clicking a disconnected one connects it. ⚙ edits settings, ↻ reconnects, ⏏ only unmounts and keeps the profile, and × removes it permanently. Network volumes that are not bookmarks appear there as well.",
          },
          {
            term: "Deleting on network shares",
            desc: "Not every network share provides a Trash; DualBeam warns you when deletion may be permanent. Large folders are deleted directly as a collection on WebDAV, and directly through rclone on S3, Swift, SFTP, and FTPS instead of one mounted file at a time. The affected folder view is refreshed automatically afterwards. SMB uses the native macOS deletion path.",
          },
          {
            term: "Insecure local protocols",
            desc: "http, ftp, afp, nfs and cifs are available only for direct private, link-local or loopback IP addresses and require an explicit warning confirmation.",
          },
          {
            term: "rclone in the background",
            desc: "macOS does not provide SFTP, FTP/FTPS, S3, or OpenStack Swift as writable drives itself. DualBeam bundles rclone and mounts these targets through the macOS NFS service. No administrator password is required. The connection ends automatically when you quit DualBeam.",
          },
        ],
      },
      {
        title: "Credentials & Keychain",
        intro:
          "WebDAV and SMB connections are mounted by macOS (Finder); DualBeam configures S3, Swift, SFTP, and FTPS through rclone. This leads to a few quirks:",
        items: [
          {
            term: "S3/Swift secrets",
            desc: "An object-storage profile holds only the non-secret connection data. The secret access key or Swift password is kept separately in the macOS Keychain. When editing a profile, leave the field empty to keep the stored secret.",
          },
          {
            term: "Password prompt",
            desc: "If you ticked “Save password” on first connect, it is stored in the macOS Keychain. macOS then mounts without asking again – this is normal and not a DualBeam bug.",
          },
          {
            term: "“webdavfs_agent…” dialog",
            desc: "This Keychain dialog comes from macOS. It does not ask for your HiDrive password, but whether the system service may read the stored password.",
          },
          {
            term: "Mounts despite “Deny”",
            desc: "If the Keychain entry has “always allow” set for webdavfs_agent, macOS reads it directly and mounts the volume in parallel. That is macOS behaviour, not a DualBeam bug.",
          },
          {
            term: "Make it ask again",
            desc: "In Keychain Access open the server’s entry → “Access Control” tab → remove webdavfs_agent or set it to “ask”. Or delete the stored entry so macOS prompts for the password on the next mount.",
          },
          {
            term: "Stop the prompt",
            desc: "To stop the recurring dialog, open Keychain Access: double-click the server’s entry → “Access Control” tab → either choose “Allow all applications to access this item” or make sure webdavfs_agent is in the list and set to “always allow” → “Save Changes”. If the dialog still appears, there is often a duplicate/old entry for the same host – delete it.",
          },
          {
            term: "NFS",
            desc: "NFS has no password prompt – access is governed by the server’s export/share list (host/IP).",
          },
        ],
      },
      {
        title: "Tips & troubleshooting",
        items: [
          {
            term: "Slow network share?",
            desc: "Directory listings and sync previews on network shares run in the background – the app stays responsive. Large folders still take time on first read.",
          },
          {
            term: "Permanent exclusions",
            desc: "A .dualbeamignore file in the source folder (one rule per line, * and ? allowed) is applied on every synchronization.",
          },
          {
            term: "Deleted instead of trashed?",
            desc: "Network shares usually provide no Trash; deleting there is permanent. DualBeam warns you beforehand.",
          },
          {
            term: "Managing favorites",
            desc: "Add the current folder via “+” in the sidebar; rename or remove via the context menu. ⌥1–⌥9 opens the first nine favorites.",
          },
          {
            term: "Something went wrong?",
            desc: "⌘Z restores the last deletion. Running jobs can be cancelled via the progress display.",
          },
          {
            term: "Learning the shortcuts",
            desc: "The function key bar at the bottom shows the most important F-keys; F1 additionally shows a quick reference in the status bar.",
          },
        ],
      },
    ],
  },
};

function ItemList(props: { items: Item[] }) {
  return (
    <dl class="help-grid">
      <For each={props.items}>
        {(item) => (
          <>
            <dt>{item.term}</dt>
            <dd>{item.desc}</dd>
          </>
        )}
      </For>
    </dl>
  );
}

export function HelpDialog() {
  function close() {
    setOpen(false);
  }

  const content = () => CONTENT[getResolvedLang() === "en" ? "en" : "de"];
  const section = () =>
    content().sections[
      Math.min(sectionIdx(), content().sections.length - 1)
    ];

  return (
    <Show when={open()}>
      <div class="modal-backdrop" onMouseDown={close}>
        <div
          class="modal help-modal"
          role="dialog"
          aria-modal="true"
          aria-label={content().title}
          onMouseDown={(e) => e.stopPropagation()}
          tabIndex={-1}
          ref={(el) => queueMicrotask(() => el?.focus())}
          onKeyDown={(ev) => {
            ev.stopPropagation();
            if (ev.key === "Escape") {
              ev.preventDefault();
              close();
            }
          }}
        >
          <h2>{content().title}</h2>
          <div class="help-layout">
            <nav class="help-nav" aria-label={content().navLabel}>
              <For each={content().sections}>
                {(s, i) => (
                  <button
                    classList={{ active: i() === sectionIdx() }}
                    onClick={() => setSectionIdx(i())}
                  >
                    {s.title}
                  </button>
                )}
              </For>
            </nav>
            <div class="help-body">
              <section class="help-section">
                <h3>{section().title}</h3>
                <Show when={section().intro}>
                  <p class="help-intro">{section().intro}</p>
                </Show>
                <Show when={section().items}>
                  <ItemList items={section().items!} />
                </Show>
                <For each={section().groups ?? []}>
                  {(group) => (
                    <div class="help-group">
                      <h4>{group.title}</h4>
                      <ItemList items={group.items} />
                    </div>
                  )}
                </For>
              </section>
            </div>
          </div>
          <div class="modal-actions">
            <button onClick={close}>{t("common.close")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
}
