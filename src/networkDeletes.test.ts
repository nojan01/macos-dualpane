import { afterEach, describe, expect, it, vi } from "vitest";
import { NetworkDeletes } from "./networkDeletes";
import type { Entry } from "./types";

const file = (path: string): Entry => ({
  path, name: path.split("/").pop()!, isDir: false, isSymlink: false,
  size: 1, mtime: 0, ext: "", hidden: false,
});

afterEach(() => vi.useRealTimers());

describe("Bestätigte Netzlaufwerks-Löschungen", () => {
  it("blendet nur bestätigte Pfade samt Unterbaum aus, nicht ähnlich benannte Nachbarn", () => {
    const deletes = new NetworkDeletes();
    deletes.confirm(["/pcloud/ordner"]);
    const remaining = file("/pcloud/ordner-alt/a.txt");
    expect(deletes.visible([file("/pcloud/ordner"), file("/pcloud/ordner/a.txt"), remaining])).toEqual([remaining]);
  });

  it("hält die Löschung über veraltete Listings hinweg und erlaubt danach eine Neuanlage", () => {
    const deletes = new NetworkDeletes();
    const old = file("/pcloud/a.txt");
    deletes.confirm([old.path]);
    expect(deletes.reconcile("/pcloud", [old], deletes.beginListing(), true)).toEqual([]);
    expect(deletes.reconcile("/pcloud", [old], deletes.beginListing(), true)).toEqual([]);
    expect(deletes.reconcile("/pcloud", [], deletes.beginListing(), true)).toEqual([]);
    expect(deletes.reconcile("/pcloud", [old], deletes.beginListing(), true)).toEqual([old]);
  });

  it("verhindert das Wiedererscheinen durch ein langsames Listing des zweiten Panes", () => {
    const deletes = new NetworkDeletes();
    const old = file("/pcloud/a.txt");
    deletes.confirm([old.path]);
    const slow = deletes.beginListing();
    deletes.reconcile("/pcloud", [], deletes.beginListing(), true);
    expect(deletes.reconcile("/pcloud", [old], slow, true)).toEqual([]);
  });

  it("verlangt eine neue Abfrage, wenn während des Listings eine Löschung bestätigt wird", () => {
    const deletes = new NetworkDeletes();
    const beforeDelete = deletes.beginListing();
    deletes.confirm(["/pcloud/a.txt"]);
    expect(deletes.reconcile("/pcloud", [], beforeDelete, true)).toBeNull();
    expect(deletes.visible([file("/pcloud/a.txt")])).toEqual([]);
  });

  it("wertet ein Listing ohne versteckte Dateien nicht als Abwesenheitsbestätigung", () => {
    const deletes = new NetworkDeletes();
    const old = file("/pcloud/.hidden");
    deletes.confirm([old.path]);
    deletes.reconcile("/pcloud", [], deletes.beginListing(), false);
    expect(deletes.reconcile("/pcloud", [old], deletes.beginListing(), true)).toEqual([]);
  });

  it("erkennt macOS-Pfadaliase und leert Marker nicht beim Lesen eines anderen Ordners", () => {
    const deletes = new NetworkDeletes();
    deletes.confirm(["/Users/test/mount/a.txt"]);
    deletes.reconcile("/other", [], deletes.beginListing(), true);
    expect(deletes.visible([file("/System/Volumes/Data/Users/test/mount/a.txt")])).toEqual([]);
  });

  it("versteckt extern neu angelegte gleichnamige Dateien nicht dauerhaft", () => {
    vi.useFakeTimers();
    const deletes = new NetworkDeletes();
    const old = file("/pcloud/a.txt");
    deletes.confirm([old.path]);
    vi.advanceTimersByTime(30_000);
    expect(deletes.reconcile("/pcloud", [old], deletes.beginListing(), true)).toEqual([]);
    vi.advanceTimersByTime(30_000);
    expect(deletes.reconcile("/pcloud", [old], deletes.beginListing(), true)).toEqual([old]);
  });
});
