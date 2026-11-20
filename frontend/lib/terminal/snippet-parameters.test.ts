import { expect, it } from "vitest";
import { snippetPreview, type Snippet } from "./snippet-parameters";

it("replaces literal matches once, including punctuation and whitespace", () => {
  const snippet: Snippet = {
    name: "literal",
    command: "{yyy} nginx /nginx NGINX [v1.0]+ first name",
    parameters: ["nginx", "[v1.0]+", "first name"].map(name => ({ name, defaultValue: null })),
  };
  const values = new Map([["nginx", "[v1.0]+"], ["[v1.0]+", "$&"], ["first name", "{yyy}"]]);
  expect(snippetPreview(snippet).map(part => part.text).join("")).toBe(snippet.command);
  expect(snippetPreview(snippet, values).map(part => part.text).join(""))
    .toBe("{yyy} [v1.0]+ /[v1.0]+ NGINX $& {yyy}");
});

it("prefers the longest name at the same position regardless of form order", () => {
  const snippet: Snippet = {
    name: "hosts", command: "web-01 web web-01",
    parameters: [{ name: "web", defaultValue: null }, { name: "web-01", defaultValue: null }],
  };
  const values = new Map([["web", "app"], ["web-01", "blue"]]);
  expect(snippetPreview(snippet, values).map(part => part.text).join("")).toBe("blue app blue");
});
