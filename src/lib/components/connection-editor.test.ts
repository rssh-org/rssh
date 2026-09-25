import { describe, expect, it } from "vitest";
import { availableConnectionKinds, connectionCopyName } from "./connection-editor";

describe("connection editor copy behavior", () => {
  it("gives every copied connection a new default name", () => {
    expect(connectionCopyName("production")).toBe("production_copy");
  });

  it("offers serial connections only when the host supports them", () => {
    expect(availableConnectionKinds(false)).toEqual(["ssh", "forward", "telnet"]);
    expect(availableConnectionKinds(true)).toEqual(["ssh", "forward", "serial", "telnet"]);
  });
});
