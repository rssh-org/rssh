import type serial from '@ohos.busManager.serial';
import type { BusinessError } from '@kit.BasicServicesKit';
import type { SerialConfig, SerialPort } from './SerialBackend';

class BusSerialPort implements SerialPort {
  readonly name: string;
  readonly key: string;
  private readonly port: serial.SerialPort;
  private opened: boolean = false;

  constructor(port: serial.SerialPort) {
    this.port = port;
    this.name = port.portInfo.portName;
    this.key = `bus:${this.name}`;
  }

  async open(config: SerialConfig, ensureActive: () => void): Promise<void> {
    ensureActive();
    const nativeConfig: serial.SerialConfigs = {
      baudRate: config.baudRate,
      dataBits: config.dataBits as serial.DataBits,
      stopBits: config.stopBits as serial.StopBits,
      parity: config.parity as serial.Parity,
      rtscts: config.flowControl === 'hardware',
      xon: config.flowControl === 'software',
      xoff: config.flowControl === 'software',
      xany: config.xany,
    };
    await this.port.open(nativeConfig);
    this.opened = true;
    // A cancelled authorization can complete late. The plugin then closes this
    // same port object before releasing its reservation.
    ensureActive();
  }

  start(onData: (data: Uint8Array) => void, onDisconnect: () => void): void {
    this.port.onDataRead(onData);
    this.port.onDisconnect(onDisconnect);
  }

  async close(): Promise<void> {
    if (!this.opened) return;
    try { await this.port.close(); }
    catch (error) {
      const code = (error as BusinessError).code;
      if (code !== 35700003 && code !== 35700005) throw error as Error;
    }
    this.opened = false;
  }

  async write(data: Uint8Array): Promise<number> { return this.port.write(data, 1000); }
  async setDtr(level: boolean): Promise<void> { await this.port.setDtr(level); }
  async setRts(level: boolean): Promise<void> { await this.port.setRts(level); }
  async sendBreak(): Promise<void> { await this.port.sendBrk(); }
}

export async function busSerialPorts(): Promise<SerialPort[]> {
  const module = await import('@ohos.busManager.serial');
  const ports = await module.default.getSerialPortList();
  return ports.map((port: serial.SerialPort) => new BusSerialPort(port));
}
