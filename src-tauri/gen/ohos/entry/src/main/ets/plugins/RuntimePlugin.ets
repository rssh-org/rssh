import deviceInfo from '@ohos.deviceInfo';
import { AsyncPluginBase, BridgeCallContext, BridgeContextRequirement, BridgeTypedValue } from '@ohos-rs/ability';

class DeviceResponse {
  readonly deviceType: string = deviceInfo.deviceType;
  readonly serial: boolean = deviceInfo.sdkApiVersion >= 26 && canIUse('SystemCapability.BusManager.Serial');
  readonly multiWindow: boolean = canIUse('SystemCapability.Window.SessionManager');
  readonly folderSelection: boolean = canIUse('SystemCapability.FileManagement.UserFileService.FolderSelection');
}

export class RuntimePlugin extends AsyncPluginBase {
  readonly id = 'rssh.runtime';
  readonly requires: BridgeContextRequirement[] = ['ability'];

  async invokeAsync(action: string, payload: BridgeTypedValue, context: BridgeCallContext): Promise<BridgeTypedValue> {
    if (!context.isActive() || context.isCancelled()) throw new Error('Runtime capability request was cancelled');
    if (action !== 'device' || payload.typeName !== 'rssh.runtime.DeviceRequest') {
      throw new Error(`Unsupported runtime action or type '${action}' / '${payload.typeName}'`);
    }
    return { typeName: 'rssh.runtime.DeviceResponse', value: new DeviceResponse() };
  }
}
