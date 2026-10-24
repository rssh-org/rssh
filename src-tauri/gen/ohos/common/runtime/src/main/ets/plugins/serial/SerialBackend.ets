import deviceInfo from '@ohos.deviceInfo';
import { busSerialPorts } from './BusSerialPort';
import { usbSerialPorts } from './UsbSerialPort';

export interface SerialConfig {
  baudRate: number;
  dataBits: number;
  parity: string;
  stopBits: number;
  flowControl: string;
  xany: boolean;
}

export interface SerialPort {
  readonly name: string;
  readonly key: string;
  open(config: SerialConfig, ensureActive: () => void): Promise<void>;
  start(onData: (data: Uint8Array) => void, onDisconnect: () => void): void;
  close(): Promise<void>;
  write(data: Uint8Array): Promise<number>;
  setDtr(level: boolean): Promise<void>;
  setRts(level: boolean): Promise<void>;
  sendBreak(): Promise<void>;
}

// Values are the API 19 BaudRates enum, not the desktop UI's shorter preset list.
const USB_BAUD_RATES: number[] = [
  50, 75, 110, 134, 150, 200, 300, 600, 1200, 1800, 2400, 4800, 9600,
  19200, 38400, 57600, 115200, 230400, 460800, 500000, 576000, 921600,
  1000000, 1152000, 1500000, 2000000, 2500000, 3000000, 3500000, 4000000,
];

export class SerialCapabilities {
  readonly flowControl: boolean;
  readonly xany: boolean;
  readonly signals: boolean;
  readonly baudRates: number[];
  constructor(extended: boolean) {
    this.flowControl = extended;
    this.xany = extended;
    this.signals = extended;
    this.baudRates = extended ? [] : USB_BAUD_RATES.slice();
  }
}

function hasBusSerial(): boolean {
  return deviceInfo.sdkApiVersion >= 26 && canIUse('SystemCapability.BusManager.Serial');
}

function hasUsbSerial(): boolean {
  return deviceInfo.sdkApiVersion >= 19 && canIUse('SystemCapability.USB.USBManager.Serial');
}

export function hasSerial(): boolean { return hasBusSerial() || hasUsbSerial(); }

export function serialCapabilities(): SerialCapabilities {
  if (!hasSerial()) throw new Error('Serial communication is not supported on this device');
  return new SerialCapabilities(hasBusSerial());
}

export function validateSerialConfig(config: SerialConfig): void {
  if (!Number.isInteger(config.baudRate) || config.baudRate <= 0 ||
    ![5, 6, 7, 8].includes(config.dataBits) || ![1, 2].includes(config.stopBits) ||
    !['none', 'odd', 'even'].includes(config.parity) ||
    !['none', 'hardware', 'software'].includes(config.flowControl) || typeof config.xany !== 'boolean') {
    throw new Error('Invalid serial configuration');
  }
  const capabilities = serialCapabilities();
  if (!capabilities.flowControl && config.flowControl !== 'none') {
    throw new Error('This serial backend does not support flow control');
  }
  if (!capabilities.xany && config.xany) throw new Error('This serial backend does not support XANY');
  if (capabilities.baudRates.length > 0 && !capabilities.baudRates.includes(config.baudRate)) {
    throw new Error(`This serial backend does not support baud rate ${config.baudRate}`);
  }
}

export async function serialPorts(): Promise<SerialPort[]> {
  // Both implementation files use type-only native imports. The unsupported
  // system module is never loaded on an older phone or computer.
  if (hasBusSerial()) return busSerialPorts();
  if (hasUsbSerial()) return usbSerialPorts();
  throw new Error('Serial communication is not supported on this device');
}
