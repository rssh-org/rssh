import { DefaultXComponent, invokeNativeBackPressInterceptor } from '@rssh/runtime';

@Entry
@Component
struct Index {
  onBackPress(): boolean {
    return invokeNativeBackPressInterceptor('rssh_ohos');
  }

  build() {
    Column() {
      DefaultXComponent({ moduleName: 'rssh_ohos' });
    }
    .width('100%')
    .height('100%');
  }
}
