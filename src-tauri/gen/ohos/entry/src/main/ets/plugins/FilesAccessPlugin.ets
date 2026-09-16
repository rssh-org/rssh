import fs from '@ohos.file.fs';
import { AsyncPluginBase, BridgeCallContext, BridgeContextRequirement, BridgeTypedValue } from '@ohos-rs/ability';

interface OpenRequest {
  uri: string;
  write: boolean;
}

class BorrowedFile {
  readonly fd: number;
  constructor(fd: number) { this.fd = fd; }
}

/** Rust validates picker grants; this adapter owns the platform File only during the callback. */
export class FilesAccessPlugin extends AsyncPluginBase {
  readonly id = 'rssh.files-access';
  readonly requires: BridgeContextRequirement[] = ['ability'];

  async invokeAsync(action: string, payload: BridgeTypedValue, context: BridgeCallContext): Promise<BridgeTypedValue> {
    if (!context.isActive() || context.isCancelled()) throw new Error('File access was cancelled');
    if (action !== 'open-file') throw new Error(`Unsupported file access action '${action}'`);
    if (payload.typeName !== 'rssh.files.OpenRequest' || !payload.value || typeof payload.value !== 'object') {
      throw new Error('open-file requires rssh.files.OpenRequest');
    }
    const request = payload.value as OpenRequest;
    if (typeof request.uri !== 'string' || !request.uri || typeof request.write !== 'boolean') {
      throw new Error('open-file requires a URI and a boolean write mode');
    }
    const mode = request.write ? fs.OpenMode.WRITE_ONLY | fs.OpenMode.TRUNC : fs.OpenMode.READ_ONLY;
    const file = fs.openSync(request.uri, mode);
    try {
      // Rust duplicates fd in this callback and returns an opaque N-API External<File>.
      // Its finalizer owns cleanup if the framework cancels or cannot decode the response.
      const owned: ESObject = context.invokeNativeSync(
        'duplicate-file', 'rssh.files.BorrowedFile', 'rssh.files.OpenedFile', new BorrowedFile(file.fd),
      );
      return { typeName: 'rssh.files.OpenedFile', value: owned };
    } finally {
      fs.closeSync(file);
    }
  }
}
