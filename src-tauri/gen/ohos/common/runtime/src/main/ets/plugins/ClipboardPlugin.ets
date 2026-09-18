import pasteboard from '@ohos.pasteboard';
import { AsyncPluginBase, BridgeCallContext, BridgeContextRequirement, BridgeTypedValue } from '@ohos-rs/ability';

interface WriteRequest { text: string; }
class WriteResponse {}
class TextResponse {
  readonly text: string;
  constructor(text: string) { this.text = text; }
}

function assertActive(context: BridgeCallContext): void {
  if (!context.isActive() || context.isCancelled()) throw new Error('Clipboard operation was cancelled');
}

/** The Rust facade requests READ_PASTEBOARD through the framework permission plugin first. */
export class ClipboardPlugin extends AsyncPluginBase {
  readonly id = 'rssh.clipboard';
  readonly requires: BridgeContextRequirement[] = ['ability'];

  async invokeAsync(action: string, payload: BridgeTypedValue, context: BridgeCallContext): Promise<BridgeTypedValue> {
    assertActive(context);
    if (!payload.value || typeof payload.value !== 'object' || Array.isArray(payload.value)) {
      throw new Error('Clipboard requests must be named objects');
    }
    if (action === 'read-text' && payload.typeName === 'rssh.clipboard.ReadRequest') {
      const data = await pasteboard.getSystemPasteboard().getData();
      assertActive(context);
      return { typeName: 'rssh.clipboard.TextResponse', value: new TextResponse(data.getPrimaryText()) };
    }
    if (action === 'write-text' && payload.typeName === 'rssh.clipboard.WriteRequest') {
      const request = payload.value as WriteRequest;
      if (typeof request.text !== 'string') throw new Error('write-text requires string text');
      await pasteboard.getSystemPasteboard().setData(pasteboard.createData(pasteboard.MIMETYPE_TEXT_PLAIN, request.text));
      assertActive(context);
      return { typeName: 'rssh.clipboard.WriteResponse', value: new WriteResponse() };
    }
    throw new Error(`Unsupported clipboard action or type '${action}' / '${payload.typeName}'`);
  }
}
