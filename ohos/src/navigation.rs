use std::sync::Mutex;

struct BackNavigation {
    state: Mutex<NavigationState>,
}

struct NavigationState {
    supported: bool,
    enabled: bool,
    revision: u64,
}

impl BackNavigation {
    const fn new() -> Self {
        Self {
            state: Mutex::new(NavigationState {
                supported: false,
                enabled: false,
                revision: 0,
            }),
        }
    }

    fn set_supported(&self, supported: bool) {
        let mut state = self.state.lock().unwrap();
        state.supported = supported;
        if !supported {
            state.enabled = false;
        }
    }

    fn set_enabled(&self, enabled: bool, revision: u64) {
        let mut state = self.state.lock().unwrap();
        if revision < state.revision {
            return;
        }
        state.revision = revision;
        state.enabled = enabled && state.supported;
    }

    fn reset(&self, revision: u64) {
        self.set_enabled(false, revision);
    }

    fn dispatch(&self, send: impl FnOnce() -> bool) -> bool {
        let enabled = {
            let state = self.state.lock().unwrap();
            state.supported && state.enabled
        };
        enabled && send()
    }
}

impl Default for BackNavigation {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn supports_system_back(device: &str, native_back: bool, window: &str) -> bool {
    native_back && matches!(device, "phone" | "tablet") && window == "main"
}

#[cfg(ohos)]
mod native {
    use std::sync::OnceLock;

    use openharmony_ability::OpenHarmonyApp;
    use tauri::{AppHandle, Emitter, Manager};

    use super::BackNavigation;

    static NAVIGATION: BackNavigation = BackNavigation::new();
    static HOST: OnceLock<AppHandle> = OnceLock::new();

    pub(crate) fn register(app: &OpenHarmonyApp) {
        // ArkUI consumes the ArkWeb keyboard and native overlays before this page
        // callback. Only the remaining app navigation reaches the frontend.
        app.on_back_press_intercept(|| {
            NAVIGATION.dispatch(|| {
                let Some(host) = HOST.get() else {
                    return false;
                };
                if host.get_webview_window("main").is_none() {
                    return false;
                }
                match host.emit_to("main", "navigation:back:main", serde_json::json!({})) {
                    Ok(()) => true,
                    Err(error) => {
                        log::warn!("Failed to dispatch system back: {error}");
                        false
                    }
                }
            })
        });
    }

    pub(crate) fn set_supported(window: &str, supported: bool) {
        if window == "main" {
            NAVIGATION.set_supported(supported);
        }
    }

    pub(crate) fn set_enabled(app: &AppHandle, window: &str, enabled: bool, revision: u64) {
        if window == "main" {
            HOST.get_or_init(|| app.clone());
            NAVIGATION.set_enabled(enabled, revision);
        }
    }

    pub(crate) fn reset() {
        NAVIGATION.reset(rssh_host::platform::host::next_back_navigation_revision());
    }
}

#[cfg(ohos)]
pub(crate) use native::{register, reset, set_enabled, set_supported};

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn unready_root_and_unsupported_hosts_pass_back_to_the_system() {
        let navigation = BackNavigation::default();
        let dispatched = Cell::new(0);
        let dispatch = || {
            dispatched.set(dispatched.get() + 1);
            true
        };
        assert!(!navigation.dispatch(dispatch));
        navigation.set_enabled(true, 1);
        assert!(!navigation.dispatch(dispatch));
        navigation.set_supported(true);
        assert!(!navigation.dispatch(dispatch));
        navigation.set_enabled(true, 2);
        assert!(navigation.dispatch(dispatch));
        navigation.set_enabled(false, 3);
        assert!(!navigation.dispatch(dispatch));
        assert_eq!(dispatched.get(), 1);
    }

    #[test]
    fn teardown_requires_the_new_frontend_to_enable_back_again() {
        let navigation = BackNavigation::default();
        navigation.set_supported(true);
        navigation.set_enabled(true, 1);
        navigation.reset(2);
        assert!(!navigation.dispatch(|| true));
        navigation.set_enabled(true, 3);
        assert!(navigation.dispatch(|| true));
        navigation.set_supported(false);
        navigation.set_supported(true);
        assert!(!navigation.dispatch(|| true));
    }

    #[test]
    fn failed_dispatch_does_not_swallow_the_system_back_action() {
        let navigation = BackNavigation::default();
        navigation.set_supported(true);
        navigation.set_enabled(true, 1);
        assert!(!navigation.dispatch(|| false));
    }

    #[test]
    fn late_page_load_reset_does_not_disable_newer_navigation() {
        let navigation = BackNavigation::default();
        navigation.set_supported(true);
        navigation.set_enabled(true, 2);
        navigation.set_enabled(false, 1);
        assert!(navigation.dispatch(|| true));
    }

    #[test]
    fn queued_enable_does_not_restore_navigation_after_lifecycle_reset() {
        let navigation = BackNavigation::default();
        navigation.set_supported(true);
        navigation.reset(2);
        navigation.set_enabled(true, 1);
        assert!(!navigation.dispatch(|| true));
        navigation.set_enabled(true, 3);
        assert!(navigation.dispatch(|| true));
    }

    #[test]
    fn only_the_mobile_host_main_window_advertises_system_back() {
        for (device, native_back, window, expected) in [
            ("phone", true, "main", true),
            ("tablet", true, "main", true),
            ("phone", false, "main", false),
            ("tablet", true, "clone", false),
            ("phone", true, "analysis", false),
            ("2in1", true, "main", false),
            ("unknown", true, "main", false),
        ] {
            assert_eq!(supports_system_back(device, native_back, window), expected);
        }
    }
}
