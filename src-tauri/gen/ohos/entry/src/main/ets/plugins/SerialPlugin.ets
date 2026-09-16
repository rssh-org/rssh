import deviceInfo from '@ohos.deviceInfo';
import type serial from '@ohos.busManager.serial';
import { AsyncPluginBase, BridgeCallContext, BridgeContextRequirement, BridgeTypedValue } from '@ohos-rs/ability';

interface OpenRequest {
  id: string;
  port: string;
  baudRate: number;
  dataBits: number;
  parity: string;
  stopBits: number;
  flowControl: string;
  xany: boolean;
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
class PendingOpen { cancelled: boolean = false; }
class SerialSession {
  readonly port: serial.SerialPort;
  closing: boolean = false;
  closeTask: Promise<void> | undefined;
  constructor(port: serial.SerialPort) { this.port = port; }
}

function ensureActive(context: BridgeCallContext): void {
  if (!context.isActive() || context.isCancelled()) throw new Error('Serial operation was cancelled');
}

/** Type-only import above emits no load on older phones. Only this capability
 * guarded path loads the API 26 native module. */
async function availablePorts(): Promise<serial.SerialPort[]> {
  if (deviceInfo.sdkApiVersion < 26 || !canIUse('SystemCapability.BusManager.Serial')) {
    throw new Error('Serial communication is not supported on this device');
  }
  const module = await import('@ohos.busManager.serial');
  return module.default.getSerialPortList();
}

export class SerialPlugin extends AsyncPluginBase {
  readonly id = 'rssh.serial';
  readonly requires: BridgeContextRequirement[] = ['ability'];
  private readonly sessions: Map<string, SerialSession> = new Map();
  private readonly pending: Map<string, PendingOpen> = new Map();
  private disposed: boolean = false;

  private output(id: string, data: number[], closed: boolean): void {
    const context = this.getContext();
    const session = this.sessions.get(id);
    if (!context.isActive() || this.disposed || !session || session.closing) return;
    context.invokeNativeSync('serial-output', 'rssh.serial.OutputEvent', 'rssh.serial.EmptyResponse', new OutputEvent(id, data, closed));
  }

  private async close(id: string): Promise<void> {
    const pending = this.pending.get(id);
    if (pending) pending.cancelled = true;
    const session = this.sessions.get(id);
    if (!session) return;
    session.closing = true;
    if (session.closeTask) return session.closeTask;
    // Retain the port until the system confirms cleanup. A failed close stays
    // unavailable to callers but can be retried during Ability disposal.
    const task = session.port.close();
    session.closeTask = task;
    try {
      await task; // The system also unregisters data and disconnect callbacks.
      this.sessions.delete(id);
    } finally {
      session.closeTask = undefined;
    }
  }

  async onDispose(): Promise<void> {
    this.disposed = true;
    for (const pending of this.pending.values()) pending.cancelled = true;
    let failure: Error | undefined;
    for (const id of Array.from(this.sessions.keys())) {
      try { await this.close(id); }
      catch (error) { failure = new Error(`Failed to close serial port: ${String(error)}`); }
    }
    if (failure) throw failure;
  }

  private async open(request: OpenRequest, context: BridgeCallContext): Promise<void> {
    if (!request.id || !request.port || this.pending.has(request.id) || this.sessions.has(request.id)) {
      throw new Error('Invalid or duplicate serial open request');
    }
    const pending = new PendingOpen();
    this.pending.set(request.id, pending);
    const removeCancellation = context.onCancel(() => { pending.cancelled = true; });
    let port: serial.SerialPort | undefined;
    let opened = false;
    try {
      const ports = await availablePorts();
      port = ports.find((entry: serial.SerialPort) => entry.portInfo.portName === request.port);
      if (!port) throw new Error('Serial port is no longer available');
      ensureActive(context);
      if (pending.cancelled || this.disposed) throw new Error('Serial operation was cancelled');
      const config: serial.SerialConfigs = {
        baudRate: request.baudRate,
        dataBits: ([5, 6, 7, 8].includes(request.dataBits) ? request.dataBits : 8) as serial.DataBits,
        stopBits: (request.stopBits === 2 ? 2 : 1) as serial.StopBits,
        parity: (request.parity === 'odd' || request.parity === 'even' ? request.parity : 'none') as serial.Parity,
        rtscts: request.flowControl === 'hardware',
        xon: request.flowControl === 'software',
        xoff: request.flowControl === 'software',
        xany: request.xany,
      };
      await port.open(config);
      opened = true;
      this.sessions.set(request.id, new SerialSession(port));
      ensureActive(context);
      if (pending.cancelled || this.disposed) throw new Error('Serial operation was cancelled');
      port.onDataRead((data: Uint8Array) => this.output(request.id, Array.from(data), false));
      port.onDisconnect(() => {
        this.output(request.id, [], true);
        this.close(request.id).catch((error: Error) => console.error(`Serial disconnect cleanup failed: ${error.message}`));
      });
    } catch (error) {
      if (opened && port) {
        try { await this.close(request.id); }
        catch (closeError) { console.error(`Serial open cleanup failed: ${String(closeError)}`); }
      }
      throw error as Error;
    } finally {
      removeCancellation();
      this.pending.delete(request.id);
    }
  }

  private async write(port: serial.SerialPort, bytes: number[], context: BridgeCallContext): Promise<void> {
    let offset = 0;
    while (offset < bytes.length) {
      ensureActive(context);
      const chunk = new Uint8Array(bytes.slice(offset, offset + 4096));
      const written = await port.write(chunk, 1000);
      if (written <= 0 || written > chunk.length) throw new Error('Serial write did not make progress');
      offset += written;
    }
  }

  async invokeAsync(action: string, payload: BridgeTypedValue, context: BridgeCallContext): Promise<BridgeTypedValue> {
    ensureActive(context);
    if (this.disposed) throw new Error('Serial host was destroyed');
    if (!payload.value || typeof payload.value !== 'object') throw new Error('Serial operation requires a request');
    if (action === 'list') {
      if (payload.typeName !== 'rssh.serial.EmptyRequest') throw new Error('Invalid serial list request');
      const ports = await availablePorts();
      ensureActive(context);
      return { typeName: 'rssh.serial.PortsResponse', value: new PortsResponse(ports.map((port: serial.SerialPort) => port.portInfo.portName)) };
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
    const port = session.port;
    if (action === 'write' && payload.typeName === 'rssh.serial.WriteRequest') {
      const write = payload.value as WriteRequest;
      if (!Array.isArray(write.data)) throw new Error('Serial write requires a byte array');
      await this.write(port, write.data, context);
    } else if ((action === 'set-dtr' || action === 'set-rts') && payload.typeName === 'rssh.serial.LevelRequest') {
      const signal = payload.value as LevelRequest;
      if (typeof signal.level !== 'boolean') throw new Error('Serial signal requires a boolean');
      if (action === 'set-dtr') await port.setDtr(signal.level);
      else await port.setRts(signal.level);
    } else if (action === 'send-break' && payload.typeName === 'rssh.serial.PortRequest') {
      await port.sendBrk();
    } else {
      throw new Error(`Unsupported serial action '${action}'`);
    }
    return { typeName: 'rssh.serial.EmptyResponse', value: new EmptyResponse() };
  }
}
