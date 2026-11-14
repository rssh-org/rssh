import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import { Terminal } from "@xterm/xterm";
import TerminalPane from "./TerminalPane.svelte";
import * as app from "../stores/app.svelte.ts";
import * as theme from "../themes/store.svelte.ts";
import * as runtime from "../stores/runtime.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";
import { installInputTracking } from "../input.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@xterm/addon-image", () => ({ ImageAddon: class { activate() {} dispose() {} } }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class {
  activate() {} dispose() {} fit() {} proposeDimensions() { return undefined; }
} }));

const tabId = "local:block-tint-test";
let component: ReturnType<typeof mount> | undefined;
let terminal: Terminal;
let target: HTMLDivElement;
let stopInputTracking: () => void;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function write(data: string) {
  await new Promise<void>((resolve) => terminal.write(data, resolve));
  await settle();
}

async function menuAction(label: string) {
  target.querySelector(".block-hit")!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
  await settle();
  const button = [...target.querySelectorAll<HTMLButtonElement>("[role=menuitem]")]
    .find((item) => item.textContent?.trim() === label);
  expect(button, label).toBeDefined();
  expect(button!.disabled).toBe(false);
  button!.click();
  await settle();
}

async function queryColors() {
  const replies: string[] = [];
  const listener = terminal.onData((data) => replies.push(data));
  await write("\x1b]4;1;?\x07\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07");
  listener.dispose();
  return replies;
}

beforeEach(async () => {
  stopInputTracking = installInputTracking();
  invokeMock.mockReset().mockImplementation(async (command, args) => {
    if (command === "pty_spawn") return args.sessionId;
    if (command === "list_highlights") return [];
    if (command === "get_runtime_capabilities") return {
      ...runtime.capabilities(),
      terminalPolicy: { imageStorageLimitMb: 128, imagePixelLimit: 16_000_000,
        outputBacklogLimitBytes: 8_000_000, gpuRenderDefault: false },
    };
    return null;
  });
  // happy-dom has no layout or canvas rasterizer. Keep xterm's real parser,
  // theme service and DOM renderer, supplying only their browser primitives.
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function () {
    return new DOMRect(0, 0, 640, this.classList.contains("xterm-screen") ? 384 : 400);
  });
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(256);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(16);
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
    clearRect() {}, fillRect() {},
  } as unknown as CanvasRenderingContext2D);
  const open = Terminal.prototype.open;
  vi.spyOn(Terminal.prototype, "open").mockImplementation(function (parent) {
    terminal = this;
    open.call(this, parent);
  });
  setLocale("en");
  await runtime.load();
  await theme.setTermGpuRender(false);
  await app.setCommandBlockBar(true);
  await app.setAutoColorBlocks(false);
  app.addTab({ id: tabId, type: "local", label: "Tint test" });
  target = document.createElement("div");
  document.body.append(target);
  component = mount(TerminalPane, { target, props: { tabId, tabType: "local", meta: {} } });
  await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("pty_spawn", expect.any(Object)));
  await settle();
  terminal.input("echo hello\r");
  await write("$ echo hello\r\nhello\r\n$ ");
  terminal.input("\r");
  await vi.waitFor(() => expect(target.querySelectorAll(".block-hit")).toHaveLength(2));
});

afterEach(async () => {
  stopInputTracking();
  if (component) await unmount(component);
  component = undefined;
  app.closeTab(tabId);
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

it.each(["touch", "pen"])("leaves %s long-press selection available on command blocks without opening a menu", async (pointerType) => {
  const block = target.querySelector(".block-hit")!;
  block.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType }));
  const events = [
    new PointerEvent("contextmenu", { bubbles: true, cancelable: true, pointerType }),
    new MouseEvent("contextmenu", { bubbles: true, cancelable: true }),
  ];
  for (const event of events) {
    block.dispatchEvent(event);
    await settle();
    expect(target.querySelector(".block-menu")).toBeNull();
    expect(event.defaultPrevented).toBe(false);
  }

  // A connected mouse still has access to the same block actions.
  block.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, pointerType: "mouse" }));
  const mouseMenu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2 });
  block.dispatchEvent(mouseMenu);
  await settle();
  expect(target.querySelector(".block-menu")).not.toBeNull();
  expect(mouseMenu.defaultPrevented).toBe(true);
});

it("preserves application OSC colors when coloring and uncoloring a block", async () => {
  await write("\x1b]4;1;#123456\x07\x1b]10;#234567\x07\x1b]11;#345678\x07\x1b]12;#456789\x07");
  const colors = await queryColors();
  expect(colors).toEqual([
    "\x1b]4;1;rgb:1212/3434/5656\x1b\\",
    "\x1b]10;rgb:2323/4545/6767\x1b\\",
    "\x1b]11;rgb:3434/5656/7878\x1b\\",
    "\x1b]12;rgb:4545/6767/8989\x1b\\",
  ]);
  await menuAction("Colorize");
  expect(target.querySelectorAll(".block-tint")).toHaveLength(1);
  expect(await queryColors()).toEqual(colors);
  await app.setCommandBlockBar(false);
  await settle();
  expect(await queryColors()).toEqual(colors);
  await app.setCommandBlockBar(true);
  await settle();
  expect(await queryColors()).toEqual(colors);
  await menuAction("Remove color");
  expect(target.querySelector(".block-tint")).toBeNull();
  expect(await queryColors()).toEqual(colors);
});

it("keeps a fully folded block reachable through its unfold menu", async () => {
  await menuAction("Fold");
  expect(target.querySelector(".fold-label")?.textContent ?? "").toContain("1 line");
  expect(target.querySelectorAll(".block-hit")).toHaveLength(2);
  await menuAction("Unfold");
  expect(target.querySelector(".fold-label")).toBeNull();
  expect(terminal.buffer.active.getLine(1)?.translateToString(true)).toBe("hello");
});
