import { describe, it, expect } from "vitest";
import { computePreviews, defaultOpReplace, defaultOpAdd, defaultOpCase } from "./rename";
import type { Entry } from "./types";

function entry(name: string): Entry {
  return {
    name,
    path: "/tmp/" + name,
    isDir: false,
    isSymlink: false,
    size: 0,
    mtime: 0,
    ext: "",
    hidden: false,
  };
}

describe("computePreviews", () => {
  it("ersetzt Text im Basisnamen", () => {
    const items = [entry("foo.txt"), entry("bar.txt")];
    const op = { ...defaultOpReplace(), scope: "base" as const, find: "o", replace: "0", mode: "all" as const };
    const res = computePreviews(items, { ops: [op] });
    expect(res[0].newName).toBe("f00.txt");
    expect(res[0].changed).toBe(true);
    expect(res[1].newName).toBe("bar.txt");
    expect(res[1].changed).toBe(false);
  });

  it("fügt ein Präfix und eine fortlaufende Nummer hinzu", () => {
    const items = [entry("a.txt"), entry("b.txt")];
    const op = {
      ...defaultOpAdd(),
      scope: "base" as const,
      prefix: "img_",
      numberPos: "after" as const,
      numberStart: 1,
      numberPad: 2,
    };
    const res = computePreviews(items, { ops: [op] });
    expect(res[0].newName).toBe("img_a01.txt");
    expect(res[1].newName).toBe("img_b02.txt");
  });

  it("erkennt doppelte Zielnamen als Fehler", () => {
    const items = [entry("a.txt"), entry("b.txt")];
    const op = { ...defaultOpCase(), scope: "base" as const, caseMode: "upper" as const };
    // Beide bleiben unterschiedlich -> kein Konflikt
    const ok = computePreviews(items, { ops: [op] });
    expect(ok.some((p) => p.error)).toBe(false);

    // Beide auf denselben Namen setzen -> Konflikt
    const collapse = { ...defaultOpReplace(), scope: "base" as const, find: ".+", replace: "same", mode: "all" as const, regex: true };
    const dup = computePreviews(items, { ops: [collapse] });
    expect(dup.every((p) => p.error)).toBe(true);
  });

  it("wendet Groß-/Kleinschreibung an", () => {
    const items = [entry("hello.TXT")];
    const op = { ...defaultOpCase(), scope: "base" as const, caseMode: "upper" as const };
    const res = computePreviews(items, { ops: [op] });
    expect(res[0].newName).toBe("HELLO.TXT");
  });
});


describe("literal replacement", () => {
  it.each(["all", "first", "start", "end", "last"] as const)("keeps dollar tokens literal in %s mode", (mode) => {
    const op = { ...defaultOpReplace(), find: "file", replace: "$&$$$1", mode };
    expect(computePreviews([entry("file")], { ops: [op] })[0].newName).toBe("$&$$$1");
  });

  it("still expands capture groups in regex mode", () => {
    const op = { ...defaultOpReplace(), find: "(file)", replace: "$1-new", regex: true };
    expect(computePreviews([entry("file")], { ops: [op] })[0].newName).toBe("file-new");
  });
});
