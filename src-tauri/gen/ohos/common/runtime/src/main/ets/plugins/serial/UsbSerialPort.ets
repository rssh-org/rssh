import type serialManager from '@ohos.usbManager.serial';
import type { BusinessError } from '@kit.BasicServicesKit';
import type { SerialConfig, SerialPort } from './SerialBackend';

interface UsbSerialApi {
  getPortList: () => Readonly<serialManager.SerialPort>[];
  hasSerialRight: (portId: number) => boolean;
  requestSerialRight: (portId: number) => Promise<boolean>;
  open: (portId: number) => void;
  close: (portId: number) => void;
  setAttribute: (portId: number, attribute: serialManager.SerialAttribute) => void;
  read: (portId: number, buffer: Uint8Array, timeout?: number) => Promise<number>;
  write: (portId: number, buffer: Uint8Array, timeout?: number) => Promise<number>;
}

function yieldRead(): Promise<void> {
  return new Promise<void>((resolve) => { setTimeout(resolve, 10); });
}

class UsbSerialPort implements SerialPort {
  readonly name: string;
  readonly key: string;
  private readonly api: UsbSerialApi;
  private readonly portId: number;
  private readonly deviceName: string;
  private opened: boolean = false;
  private stopping: boolean = false;
  private reader: Promise<void> | undefined;

  constructor(api: UsbSerialApi, port: Readonly<serialManager.SerialPort>, name: string) {
    this.api = api;
    this.portId = port.portId;
    this.deviceName = port.deviceName;
    this.name = name;
    this.key = `usb:${port.portId}`;
  }

  private available(): boolean {
    return this.api.getPortList().some((port: Readonly<serialManager.SerialPort>) =>
      port.portId === this.portId && port.deviceName === this.deviceName);
  }

  async open(config: SerialConfig, ensureActive: () => void): Promise<void> {
    ensureActive();
    if (!this.api.hasSerialRight(this.portId) && !await this.api.requestSerialRight(this.portId)) {
      throw new Error('Serial port authorization was denied');
    }
    ensureActive();
    if (!this.available()) throw new Error('Serial port is no longer available');
    this.api.open(this.portId);
    this.opened = true;
    const attribute: serialManager.SerialAttribute = {
      baudRate: config.baudRate as serialManager.BaudRates,
      dataBits: config.dataBits as serialManager.DataBits,
      parity: (config.parity === 'odd' ? 1 : config.parity === 'even' ? 2 : 0) as serialManager.Parity,
      // API 19 uses 0/1 enum values, unlike the API 26 1/2 values.
      stopBits: (config.stopBits === 2 ? 1 : 0) as serialManager.StopBits,
    };
    this.api.setAttribute(this.portId, attribute);
  }

  start(onData: (data: Uint8Array) => void, onDisconnect: () => void): void {
    this.reader = this.read(onData, onDisconnect);
  }

  private async read(onData: (data: Uint8Array) => void, onDisconnect: () => void): Promise<void> {
    const buffer = new Uint8Array(8192);
    while (!this.stopping) {
      try {
        const length = await this.api.read(this.portId, buffer, 250);
        if (this.stopping) return;
        if (!Number.isInteger(length) || length < 0 || length > buffer.length) {
          throw new Error('Serial read returned an invalid byte count');
        }
        if (length > 0) onData(buffer.slice(0, length));
        if (length === 0 && !this.available()) throw new Error('Serial port was disconnected');
        if (length === 0) await yieldRead();
      } catch (error) {
        if (this.stopping) return;
        if ((error as BusinessError).code === 31400006) {
          // An idle port may report timeout as an error. Yield even if the
          // native call rejects immediately, and still detect unplugged ports.
          try {
            if (this.available()) { await yieldRead(); continue; }
          } catch (enumerationError) {
            console.error(`Serial enumeration failed: ${String(enumerationError)}`);
          }
        }
        console.error(`Serial read failed: ${String(error)}`);
        onDisconnect();
        return;
      }
    }
  }

  async close(): Promise<void> {
    this.stopping = true;
    // Never close/reopen the numeric port while its old read is in flight.
    // read() is asynchronous and bounded, so the UI thread stays responsive.
    if (this.reader) await this.reader;
    if (!this.opened) return;
    try { this.api.close(this.portId); }
    catch (error) {
      const code = (error as BusinessError).code;
      if (code !== 31400003 && code !== 31400005) throw error as Error;
    }
    this.opened = false;
  }

  async write(data: Uint8Array): Promise<number> { return this.api.write(this.portId, data, 1000); }
  async setDtr(_level: boolean): Promise<void> { throw new Error('This serial backend does not support DTR'); }
  async setRts(_level: boolean): Promise<void> { throw new Error('This serial backend does not support RTS'); }
  async sendBreak(): Promise<void> { throw new Error('This serial backend does not support BREAK'); }
}

export async function usbSerialPorts(): Promise<SerialPort[]> {
  const module = await import('@ohos.usbManager.serial');
  // ArkTS namespaces cannot be assigned as objects. Keep the delayed import
  // while adapting its functions to the port's explicit API dependency.
  const api: UsbSerialApi = {
    getPortList: () => module.default.getPortList(),
    hasSerialRight: (id: number) => module.default.hasSerialRight(id),
    requestSerialRight: (id: number) => module.default.requestSerialRight(id),
    open: (id: number) => module.default.open(id),
    close: (id: number) => module.default.close(id),
    setAttribute: (id: number, attribute: serialManager.SerialAttribute) => module.default.setAttribute(id, attribute),
    read: (id: number, buffer: Uint8Array, timeout?: number) => module.default.read(id, buffer, timeout),
    write: (id: number, buffer: Uint8Array, timeout?: number) => module.default.write(id, buffer, timeout),
  };
  const ports = api.getPortList();
  return ports.map((port: Readonly<serialManager.SerialPort>) => {
    const unique = port.deviceName.length > 0 && ports.filter((entry: Readonly<serialManager.SerialPort>) =>
      entry.deviceName === port.deviceName).length === 1;
    // These are system identifiers, never file paths to open directly. Include
    // the port ID only when the device name does not uniquely identify a port.
    const name = unique ? port.deviceName : `ohos-usb-serial://${port.portId}/${encodeURIComponent(port.deviceName)}`;
    return new UsbSerialPort(api, port, name);
  });
}
