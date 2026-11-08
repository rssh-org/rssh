import { NativeAbility } from '@ohos-rs/ability';
import { window } from '@kit.ArkUI';
import { rsshPlugins } from './RsshPlugins';

export class RsshAbility extends NativeAbility {
  public moduleName: string = 'rssh_ohos';
  public bridgePlugins = rsshPlugins(true);

  protected override async loadWindowStageContent(windowStage: window.WindowStage): Promise<void> {
    // This mobile HAP serves both phones and tablets; desktop has its own Ability.
    // ArkWeb exposes cutout insets to CSS without reserving a native top strip.
    // Keep keyboard and navigation-bar avoidance in ArkUI.
    try {
      const mainWindow = await windowStage.getMainWindow();
      await mainWindow.setSpecificSystemBarEnabled('status', false);
    } catch (error) {
      console.error(`[RsshAbility] Failed to hide mobile status bar: ${String(error)}`);
    }
    await windowStage.loadContent('pages/Index');
  }

  protected override createInitContext(moduleName: string) {
    const context = super.createInitContext(moduleName);
    // Keep existing databases and keys in ApplicationContext's /base/files.
    // https://github.com/openharmony/docs/blob/master/en/application-dev/application-models/application-context-stage.md#obtaining-application-file-paths
    const filesDir = this.context.getApplicationContext().filesDir;
    context.basePath = filesDir;
    context.prefPath = filesDir;
    return context;
  }
}
