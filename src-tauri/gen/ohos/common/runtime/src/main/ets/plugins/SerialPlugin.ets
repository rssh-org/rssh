import type { BusinessError } from '@kit.BasicServicesKit';
import { AsyncPluginBase, BridgeCallContext, BridgeContextRequirement, BridgeTypedValue } from '@ohos-rs/ability';
import { SerialConfig, SerialPort, serialCapabilities, serialPorts, validateSerialConfig } from './serial/SerialBackend';

interface OpenRequest extends SerialConfig {
  id: string;
  port: string;
}
interface PortRequest { id: string; }
interface WriteRequest { id: string; data: number[]; }
interface LevelRequest { id: string; level: boolean; }
class EmptyResponse {}
class PortsResponse {
  readonly ports: string[];
  constructor(ports: string[]) { this.ports = ports; }
}
class OutputEvent {
  readonly id: string;
  readonly data: number[];
  readonly closed: boolean;
  constructor(id: string, data: number[], closed: boolean) {
    this.id = id;
    this.data = data;
    this.closed = closed;
  }
}
class PendingOpen {
  cancelled: boolean = false;
  readonly finished: Promise<void>;
  private settle: () => void = () => {};
  constructor() {
    this.finished = new Promise<void>((resolve) => { this.settle = resolve; });
  }
  finish(): void { this.settle(); }
}
class SerialSession {
  readonly id: string;
  readonly port: SerialPort;
  closing: boolean = false;
  disconnected: boolean = false;
  closeTask: Promise<void> | undefined;
  operations: Promise<void> = Promise.resolve();
  constructor(id: string, port: SerialPort) { this.id = id; this.port = port; }
}

function ensureActive(context: BridgeCallContext): void {
  if (!context.isActive() || context.isCancelled()) throw new Error('Serial operation was cancelled');
}

function isDisconnected(error: BusinessError): boolean {
  return [31400003, 31400005, 31400007, 35700003, 35700005].includes(error.code);
}

export class SerialPlugin extends AsyncPluginBase {
  readonly id = 'rssh.serial';
  readonly requires: BridgeContextRequirement[] = ['ability'];
  private readonly sessions: Map<string, SerialSession> = new Map();
  private readonly pending: Map<string, PendingOpen> = new Map();
  private readonly reservedPorts: Map<string, string> = new Map();
  private disposed: boolean = false;

  private output(session: SerialSession, data: number[], closed: boolean): void {
    const context = this.getContext();
    if (!context.isActive() || this.disposed || this.sessions.get(session.id) !== session || session.closing) return;
    context.invokeNativeSync('serial-output', 'rssh.serial.OutputEvent', 'rssh.serial.EmptyResponse', new OutputEvent(session.id, data, closed));
  }

  private disconnect(session: SerialSession): void {
    if (session.closing || session.disconnected) return;
    session.disconnected = true;
    try { this.output(session, [], true); }
    catch (error) { console.error(`Serial disconnect notification failed: ${String(error)}`); }
    this.closeSession(session).catch((error: Error) => console.error(`Serial disconnect cleanup failed: ${error.message}`));
  }

  private async finishClose(session: SerialSession): Promise<void> {
    // No native close races an in-flight write or signal operation. API 19 also
    // joins its read loop before releasing the numeric device port.
    await session.operations;
    await session.port.close();
    if (this.sessions.get(session.id) === session) this.sessions.delete(session.id);
    if (this.reservedPorts.get(session.port.key) === session.id) this.reservedPorts.delete(session.port.key);
  }

  private async closeSession(session: SerialSession): Promise<void> {
    session.closing = true;
    if (session.closeTask) return session.closeTask;
    const task = this.finishClose(session);
    session.closeTask = task;
    try { await task; }
    finally { session.closeTask = undefined; }
    // A failed close deliberately retains the reservation for disposal/retry.
  }

  private async close(id: string): Promise<void> {
    const pending = this.pending.get(id);
    if (pending) {
      pending.cancelled = true;
      // System authorization cannot be dismissed programmatically. A close
      // acknowledges release only after a late authorization is cleaned up.
      await pending.finished;
    }
    const session = this.sessions.get(id);
    if (session) await this.closeSession(session);
  }

  async onDispose(): Promise<void> {
    this.disposed = true;
    for (const pending of this.pending.values()) pending.cancelled = true;
    let failure: Error | undefined;
    // Teardown must not wait for a human authorization dialog. Pending opens
    // observe disposed/cancelled and perform their own late-result cleanup.
    for (const session of Array.from(this.sessions.values())) {
      try { await this.closeSession(session); }
      catch (error) { failure = new Error(`Failed to close serial port: ${String(error)}`); }
    }
    if (failure) throw failure;
  }

  private async open(request: OpenRequest, context: BridgeCallContext): Promise<void> {
    if (typeof request.id !== 'string' || !request.id || typeof request.port !== 'string' || !request.port ||
      this.pending.has(request.id) || this.sessions.has(request.id)) {
      throw new Error('Invalid or duplicate serial open request');
    }
    validateSerialConfig(request);
    const pending = new PendingOpen();
    this.pending.set(request.id, pending);
    const removeCancellation = context.onCancel(() => { pending.cancelled = true; });
    const checkActive = (): void => {
      ensureActive(context);
      if (pending.cancelled || this.disposed) throw new Error('Serial operation was cancelled');
    };
    let session: SerialSession | undefined;
    try {
      const ports = await serialPorts();
      checkActive();
      const port = ports.find((entry: SerialPort) => entry.name === request.port);
      if (!port) throw new Error('Serial port is no longer available');
      if (this.reservedPorts.has(port.key)) throw new Error('Serial port is already in use');
      this.reservedPorts.set(port.key, request.id);
      const opening = new SerialSession(request.id, port);
      session = opening;
      await port.open(request, checkActive);
      this.sessions.set(request.id, opening);
      checkActive();
      port.start((data: Uint8Array) => this.output(opening, Array.from(data), false), () => this.disconnect(opening));
      if (opening.disconnected || opening.closing) throw new Error('Serial port was disconnected while opening');
    } catch (error) {
      if (session) {
        // Include a partially opened API 19 port (setAttribute can fail) and a
        // successful API 26 authorization that arrived after cancellation.
        this.sessions.set(request.id, session);
        try { await this.closeSession(session); }
        catch (closeError) { console.error(`Serial open cleanup failed: ${String(closeError)}`); }
      }
      throw error as Error;
    } finally {
      try { removeCancellation(); }
      finally {
        this.pending.delete(request.id);
        pending.finish();
      }
    }
  }

  private async write(session: SerialSession, bytes: number[], context: BridgeCallContext): Promise<void> {
    let offset = 0;
    while (offset < bytes.length) {
      ensureActive(context);
      if (session.closing) throw new Error('Serial session is closed');
      const chunk = new Uint8Array(bytes.slice(offset, offset + 4096));
      const written = await session.port.write(chunk);
      if (!Number.isInteger(written) || written <= 0 || written > chunk.length) throw new Error('Serial write did not make progress');
      offset += written;
    }
  }

  private async operate(session: SerialSession, context: BridgeCallContext, operation: () => Promise<void>): Promise<void> {
    const task = session.operations.then(async () => {
      ensureActive(context);
      if (session.closing || this.disposed) throw new Error('Serial session is closed');
      await operation();
    });
    // This tail only tracks completion; the original task reports failures to
    // its caller. Writes remain contiguous even when multiple commands arrive.
    session.operations = task.then(() => {}, () => {});
    try { await task; }
    catch (error) {
      if (isDisconnected(error as BusinessError)) this.disconnect(session);
      throw error as Error;
    }
  }

  async invokeAsync(action: string, payload: BridgeTypedValue, context: BridgeCallContext): Promise<BridgeTypedValue> {
    ensureActive(context);
    if (this.disposed) throw new Error('Serial host was destroyed');
    if (!payload.value || typeof payload.value !== 'object') throw new Error('Serial operation requires a request');
    if (action === 'capabilities') {
      if (payload.typeName !== 'rssh.serial.EmptyRequest') throw new Error('Invalid serial capability request');
      return { typeName: 'rssh.serial.CapabilitiesResponse', value: serialCapabilities() };
    }
    if (action === 'list') {
      if (payload.typeName !== 'rssh.serial.EmptyRequest') throw new Error('Invalid serial list request');
      const ports = await serialPorts();
      ensureActive(context);
      return { typeName: 'rssh.serial.PortsResponse', value: new PortsResponse(ports.map((port: SerialPort) => port.name)) };
    }
    if (action === 'open') {
      if (payload.typeName !== 'rssh.serial.OpenRequest') throw new Error('Invalid serial open request');
      await this.open(payload.value as OpenRequest, context);
      return { typeName: 'rssh.serial.EmptyResponse', value: new EmptyResponse() };
    }
    const request = payload.value as PortRequest;
    if (typeof request.id !== 'string' || !request.id) throw new Error('Serial operation requires an open attempt ID');
    if (action === 'close') {
      if (payload.typeName !== 'rssh.serial.PortRequest') throw new Error('Invalid serial close request');
      await this.close(request.id);
      return { typeName: 'rssh.serial.EmptyResponse', value: new EmptyResponse() };
    }
    const session = this.sessions.get(request.id);
    if (!session || session.closing) throw new Error('Serial session is closed');
    if (action === 'write' && payload.typeName === 'rssh.serial.WriteRequest') {
      const write = payload.value as WriteRequest;
      if (!Array.isArray(write.data) || write.data.some((byte: number) => !Number.isInteger(byte) || byte < 0 || byte > 255)) {
        throw new Error('Serial write requires a byte array');
      }
      await this.operate(session, context, () => this.write(session, write.data, context));
    } else if ((action === 'set-dtr' || action === 'set-rts') && payload.typeName === 'rssh.serial.LevelRequest') {
      const signal = payload.value as LevelRequest;
      if (typeof signal.level !== 'boolean') throw new Error('Serial signal requires a boolean');
      await this.operate(session, context, () => action === 'set-dtr' ? session.port.setDtr(signal.level) : session.port.setRts(signal.level));
    } else if (action === 'send-break' && payload.typeName === 'rssh.serial.PortRequest') {
      await this.operate(session, context, () => session.port.sendBreak());
    } else {
      throw new Error(`Unsupported serial action '${action}'`);
    }
    return { typeName: 'rssh.serial.EmptyResponse', value: new EmptyResponse() };
  }
}
