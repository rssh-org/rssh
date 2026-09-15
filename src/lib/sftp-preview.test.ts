import { describe, expect, it } from "vitest";
import {
    cmLanguageFor, decodeText, mediaMime, parseDelimited, previewKind, serializeDelimited,
} from "./sftp-preview";

describe("previewKind", () => {
    it("classifies media", () => {
        expect(previewKind("fig_manhattan.png")).toBe("image");
        expect(previewKind("chr1.jpg")).toBe("image");
        expect(previewKind("report.pdf")).toBe("pdf");
        expect(previewKind("plot.svg")).toBe("image");
    });

    it("classifies markdown / tables / text / code", () => {
        expect(previewKind("README.md")).toBe("markdown");
        expect(previewKind("samples.tsv")).toBe("table");
        expect(previewKind("phenotype.csv")).toBe("table");
        expect(previewKind("run_pipeline.sh")).toBe("code");
        expect(previewKind("plot.R")).toBe("code");
        expect(previewKind("qc.py")).toBe("code");
        expect(previewKind("samples.log")).toBe("code");
        expect(previewKind("notes.txt")).toBe("code");
        expect(previewKind("sample1.fastq")).toBe("code");
        expect(previewKind("variants.vcf")).toBe("code");
        expect(previewKind("genes.gff")).toBe("code");
    });

    it("rejects unsupported and extension-less names", () => {
        expect(previewKind("noext")).toBeNull();
        expect(previewKind("archive.tar.gz")).toBeNull();
        expect(previewKind("data.bin")).toBeNull();
    });
});

describe("cmLanguageFor", () => {
    it("maps to legacy modes", () => {
        expect(cmLanguageFor("a.sh")).toBe("shell");
        expect(cmLanguageFor("a.bash")).toBe("shell");
        expect(cmLanguageFor("a.py")).toBe("python");
        expect(cmLanguageFor("a.R")).toBe("r");
        expect(cmLanguageFor("a.r")).toBe("r");
        expect(cmLanguageFor("a.pl")).toBe("perl");
        expect(cmLanguageFor("a.rs")).toBe("rust");
        expect(cmLanguageFor("a.go")).toBe("go");
        expect(cmLanguageFor("a.js")).toBe("javascript");
        expect(cmLanguageFor("a.json")).toBe("javascript");
        expect(cmLanguageFor("a.c")).toBe("clike");
        expect(cmLanguageFor("a.cpp")).toBe("clike");
        expect(cmLanguageFor("a.html")).toBe("xml");
        expect(cmLanguageFor("a.yaml")).toBe("yaml");
        expect(cmLanguageFor("a.toml")).toBe("toml");
        expect(cmLanguageFor("a.ini")).toBe("properties");
        expect(cmLanguageFor("a.ps1")).toBe("powershell");
    });

    it("falls back to plaintext", () => {
        expect(cmLanguageFor("a.tsv")).toBe("plaintext");
        expect(cmLanguageFor("a.log")).toBe("plaintext");
        expect(cmLanguageFor("a.fastq")).toBe("plaintext");
    });
});

describe("parseDelimited", () => {
    it("parses csv with quoted fields and escaped quotes", () => {
        const rows = parseDelimited('a,b\n"x,y",z\n"q""q",w\n', ",");
        expect(rows).toEqual([["a", "b"], ["x,y", "z"], ['q"q', "w"]]);
    });

    it("parses tsv", () => {
        expect(parseDelimited("a\tb\n1\t2\n", "\t")).toEqual([["a", "b"], ["1", "2"]]);
    });

    it("handles windows line endings", () => {
        expect(parseDelimited("a,b\r\n1,2\r\n", ",")).toEqual([["a", "b"], ["1", "2"]]);
    });
});

describe("serializeDelimited", () => {
    it("round-trips simple csv", () => {
        const rows = parseDelimited("a,b\n1,2\n", ",");
        expect(serializeDelimited(rows, ",")).toBe("a,b\n1,2\n");
    });

    it("quotes fields containing delimiter, quotes or newlines", () => {
        expect(serializeDelimited([["x,y", 'q"q', "a\nb"]], ",")).toBe('"x,y","q""q","a\nb"\n');
    });

    it("round-trips tsv", () => {
        const rows = parseDelimited("a\tb\n1\t2\n", "\t");
        expect(serializeDelimited(rows, "\t")).toBe("a\tb\n1\t2\n");
    });

    it("returns empty string for no rows", () => {
        expect(serializeDelimited([], ",")).toBe("");
    });
});

describe("decodeText", () => {
    it("strips utf-8 BOM", () => {
        const u8 = new Uint8Array([0xef, 0xbb, 0xbf, 0x61, 0x62]);
        expect(decodeText(u8)).toBe("ab");
    });

    it("decodes plain utf-8", () => {
        const u8 = new TextEncoder().encode("你好\nline2");
        expect(decodeText(u8)).toBe("你好\nline2");
    });
});

describe("mediaMime", () => {
    it("maps mime types", () => {
        expect(mediaMime("a.pdf")).toBe("application/pdf");
        expect(mediaMime("a.png")).toBe("image/png");
        expect(mediaMime("a.jpg")).toBe("image/jpeg");
        expect(mediaMime("a.svg")).toBe("image/svg+xml");
    });
});
