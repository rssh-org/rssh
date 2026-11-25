import webview from '@ohos.web.webview';
import pasteboard from '@ohos.pasteboard';
import { WebviewEditMenu } from '@ohos-rs/ability-plugin-webview';

// ArkWeb's native menu treats xterm output as read-only DOM. Cache the touch
// target before the synchronous menu callback; clipboard reads still use RSSH's
// existing permission-aware path, and happen only after the user taps Paste.
export class TerminalEditMenu implements WebviewEditMenu {
  private generation: number = 0;
  private sessionId: string = '';
  private menuSessionId: string = '';
  private addedPasteSessionId: string = '';
  private controller: webview.WebviewController;

  readonly options: EditMenuOptions = {
    onCreateMenu: (items: TextMenuItem[]): TextMenuItem[] => this.createMenu(items),
    onPrepareMenu: (items: TextMenuItem[]): TextMenuItem[] => this.createMenu(items, true),
    onMenuItemClick: (item: TextMenuItem, _range: TextRange): boolean => this.clickMenu(item),
  };

  constructor(controller: webview.WebviewController) {
    this.controller = controller;
  }

  reset(): void {
    this.generation += 1;
    this.sessionId = '';
    this.menuSessionId = '';
    this.addedPasteSessionId = '';
  }

  onTouch(event: TouchEvent): void {
    if (event.type !== TouchType.Down || event.touches.length === 0) return;
    this.generation += 1;
    this.sessionId = '';
    this.menuSessionId = '';
    const generation = this.generation;
    const point = event.touches[0];
    // Touch coordinates are Web-local vp, matching CSS pixels with our viewport.
    const script = `(() => {
      const x = ${point.x}, y = ${point.y};
      const terminalHost = (node) => {
        const el = node instanceof Element ? node : node?.parentElement;
        if (!el || el.closest('input,textarea') || el.isContentEditable) return null;
        const host = el.closest('.xterm-host[data-terminal-session]');
        if (!host?.isConnected || !host.getClientRects().length) return null;
        const style = getComputedStyle(host);
        return style.display !== 'none' && style.visibility !== 'hidden' ? host : null;
      };
      const hit = document.elementFromPoint(x, y);
      let host = terminalHost(hit);
      // Native selection handles may extend slightly outside the DOM host.
      if (!host && !hit?.closest('input,textarea,button,a,[role="button"]') && !hit?.isContentEditable) {
        const selection = window.getSelection();
        const selected = terminalHost(selection?.anchorNode);
        if (selected && selected === terminalHost(selection?.focusNode) &&
            !selection.isCollapsed && selection.rangeCount) {
          const rect = selection.getRangeAt(0).getBoundingClientRect();
          if (x >= rect.left - 24 && x <= rect.right + 24 &&
              y >= rect.top - 24 && y <= rect.bottom + 24) host = selected;
        }
      }
      return host?.dataset.terminalSession ?? '';
    })()`;
    this.controller.runJavaScript(script).then((result: string) => {
      if (generation !== this.generation) return;
      const sessionId: string = JSON.parse(result);
      this.sessionId = typeof sessionId === 'string' ? sessionId : '';
    }).catch((error: Error) => {
      if (generation === this.generation) console.error(`Terminal menu target failed: ${String(error)}`);
    });
  }

  private createMenu(items: TextMenuItem[], preparing: boolean = false): TextMenuItem[] {
    this.menuSessionId = '';
    if (!preparing) this.addedPasteSessionId = '';
    if (!this.sessionId) return items;
    if (items.some((item: TextMenuItem) => item.id.equals(TextMenuItemId.PASTE))) {
      if (preparing && this.addedPasteSessionId === this.sessionId) this.menuSessionId = this.sessionId;
      return items;
    }
    try {
      const clipboard = pasteboard.getSystemPasteboard();
      if (!clipboard.hasDataType(pasteboard.MIMETYPE_TEXT_PLAIN) &&
          !clipboard.hasDataType(pasteboard.MIMETYPE_TEXT_HTML)) return items;
    } catch (error) {
      console.error(`Terminal menu clipboard metadata failed: ${String(error)}`);
      return items;
    }
    this.menuSessionId = this.sessionId;
    this.addedPasteSessionId = this.sessionId;
    return [{ id: TextMenuItemId.PASTE, content: '' }, ...items];
  }

  private clickMenu(item: TextMenuItem): boolean {
    if (!item.id.equals(TextMenuItemId.PASTE) || !this.menuSessionId) return false;
    const sessionId = this.menuSessionId;
    this.menuSessionId = '';
    this.addedPasteSessionId = '';
    const script = `(() => {
      const sessionId = ${JSON.stringify(sessionId)};
      const host = Array.from(document.querySelectorAll('.xterm-host[data-terminal-session]'))
        .find(el => el.dataset.terminalSession === sessionId && el.isConnected &&
          el.getClientRects().length && getComputedStyle(el).display !== 'none' &&
          getComputedStyle(el).visibility !== 'hidden');
      const selection = window.getSelection();
      if (selection && !selection.isCollapsed &&
          (!host?.contains(selection.anchorNode) || !host.contains(selection.focusNode))) return;
      if (host) host.dispatchEvent(new CustomEvent('terminal:paste:' + sessionId,
        { detail: { readClipboard: true } }));
    })()`;
    this.controller.runJavaScript(script).catch((error: Error) => {
      console.error(`Terminal menu paste failed: ${String(error)}`);
    });
    return true;
  }
}
