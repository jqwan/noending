//! Match the display's refresh rate instead of WKWebView's default 60 FPS
//! preference. xterm's renderer uses requestAnimationFrame, so this affects
//! terminal scrolling as well as the rest of the webview.
//!
//! WebKit has no public embedder API for this preference yet:
//! https://bugs.webkit.org/show_bug.cgi?id=294338
//! Keep the private API isolated here and leave defaults intact when either
//! the selectors or the feature are absent. Do not guess by macOS version.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSString};

pub fn prefer_native_refresh_rate(window: &tauri::WebviewWindow) {
    // Tauri dispatches this callback on the UI thread. The native view pointer
    // remains valid throughout the callback; no pointer escapes it.
    if let Err(error) = window.with_webview(|view| unsafe {
        let Some(preferences_class) = AnyClass::get(c"WKPreferences") else {
            return;
        };
        let has_features: bool = msg_send![preferences_class, respondsToSelector: sel!(_features)];
        if !has_features {
            return;
        }
        let webview = &*view.inner().cast::<AnyObject>();
        let configuration: Retained<AnyObject> = msg_send![webview, configuration];
        let preferences: Retained<AnyObject> = msg_send![&*configuration, preferences];
        let has_setter: bool =
            msg_send![&*preferences, respondsToSelector: sel!(_setEnabled:forFeature:)];
        if !has_setter {
            return;
        }
        let features: Option<Retained<NSArray<AnyObject>>> =
            msg_send![preferences_class, _features];
        let Some(features) = features else {
            return;
        };
        for feature in features.iter() {
            let has_key: bool = msg_send![&*feature, respondsToSelector: sel!(key)];
            if !has_key {
                continue;
            }
            let key: Option<Retained<NSString>> = msg_send![&*feature, key];
            if key.as_deref().map(NSString::to_string).as_deref()
                == Some("PreferPageRenderingUpdatesNear60FPSEnabled")
            {
                let _: () = msg_send![&*preferences, _setEnabled: false, forFeature: &*feature];
                break;
            }
        }
    }) {
        eprintln!("[noending] could not configure webview refresh rate: {error}");
    }
}
