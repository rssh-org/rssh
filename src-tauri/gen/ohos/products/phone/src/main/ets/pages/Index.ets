import { DefaultXComponent, invokeNativeBackPressInterceptor } from '@rssh/runtime';

@Entry
@Component
struct Index {
  onBackPress(): boolean {
    return invokeNativeBackPressInterceptor('rssh_lib');
  }

  build() {
    Column() {
      DefaultXComponent({ moduleName: 'rssh_lib' });
    }
    .width('100%')
    .height('100%');
  }
}
