import { RustAbility } from '@ohos-rs/ability'
import Want from '@ohos.app.ability.Want'
import { AbilityConstant, abilityAccessCtrl, Permissions } from '@kit.AbilityKit';
import window from '@ohos.window';

export default class EntryAbility extends RustAbility {
  // change to dynamic library name
  public moduleName: string = "rssh_lib"
  public defaultPage: boolean = true;
  public mode: 'xcomponent' | 'webview' = 'webview'

  async onCreate(want: Want, launchParam: AbilityConstant.LaunchParam): Promise<void> {
    super.onCreate(want, launchParam);
    // SFTP staging lives in the public Downloads dir (see commands/sftp.rs).
    // One-time grant prompt at startup; denial degrades transfers to a clear
    // error from the pick commands instead of a silent failure.
    try {
      const atManager = abilityAccessCtrl.createAtManager();
      const perms: Array<Permissions> = ['ohos.permission.READ_WRITE_DOWNLOAD_DIRECTORY'];
      atManager.requestPermissionsFromUser(this.context, perms).catch(() => {});
    } catch (e) {
      // Non-fatal: transfers surface a clear error without the grant.
    }
  }

  async onWindowStageCreate(windowStage: window.WindowStage): Promise<void> {
    const window = windowStage.getMainWindowSync();
    await window.setWindowLayoutFullScreen(false);
    super.onWindowStageCreate(windowStage);
  }
}
