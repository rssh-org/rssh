import { hapTasks } from '@ohos/hvigor-ohos-plugin';

export default {
  system: hapTasks,
  /* cargo hook removed: librssh_lib.so is cross-compiled by build-ohos.sh
     (cargo tauri ohos build) and already staged in entry/libs/arm64-v8a/.
     The hook's dev-eco-studio-script needs the cargo-mobile2 daemon and is
     the documented Windows blocker anyway. */
  plugins: []
}
