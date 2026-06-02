import { describe, it, expect } from "vitest";
import { joinPath, splitName } from "./paths";

describe("joinPath", () => {
  it("fügt einen Separator ein, wenn keiner vorhanden ist", () => {
    expect(joinPath("/Users/foo", "bar.txt")).toBe("/Users/foo/bar.txt");
  });

  it("verdoppelt einen vorhandenen Separator nicht", () => {
    expect(joinPath("/Users/foo/", "bar.txt")).toBe("/Users/foo/bar.txt");
  });

  it("funktioniert für den Root-Pfad", () => {
    expect(joinPath("/", "bar.txt")).toBe("/bar.txt");
  });
});

describe("splitName", () => {
  it("trennt Basisname und Endung", () => {
    expect(splitName("foto.png")).toEqual({ base: "foto", ext: ".png" });
  });

  it("behandelt mehrere Punkte korrekt (letzter zählt)", () => {
    expect(splitName("archiv.tar.gz")).toEqual({ base: "archiv.tar", ext: ".gz" });
  });

  it("behandelt versteckte Dateien ohne Endung", () => {
    expect(splitName(".gitignore")).toEqual({ base: ".gitignore", ext: "" });
  });

  it("behandelt Namen ohne Endung", () => {
    expect(splitName("README")).toEqual({ base: "README", ext: "" });
  });

  it("behandelt einen abschließenden Punkt als Teil der Basis", () => {
    expect(splitName("name.")).toEqual({ base: "name.", ext: "" });
  });
});
