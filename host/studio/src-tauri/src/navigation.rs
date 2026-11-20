//! Opening Studio at a page.
//!
//! The tray and other launchers start Studio with `--page <name>`. When Studio is already
//! running, the single-instance plugin hands the arguments to the running instance, which
//! brings its window forward and switches to the page.

use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime, State};

/// Pages a launcher may ask for.
pub const PAGES: &[&str] = &[
    "updates",
    "apps",
    "themes",
    "keymap",
    "actuation",
    "mosaic",
    "profiles",
    "named_profiles",
    "firmware",
];

/// The page named by `--page <name>` or `--page=<name>`, if it is one Studio has.
pub fn page_from_args<I, S>(args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let a = a.as_ref();
        let value = if a == "--page" {
            it.next().map(|v| v.as_ref().to_string())
        } else {
            a.strip_prefix("--page=").map(str::to_string)
        };
        if let Some(v) = value {
            return PAGES.contains(&v.as_str()).then_some(v);
        }
    }
    None
}

#[derive(Default)]
pub struct NavigationState {
    initial: Mutex<Option<String>>,
    listener: Mutex<Option<Channel<String>>>,
}

impl NavigationState {
    pub fn with_initial(page: Option<String>) -> Self {
        Self {
            initial: Mutex::new(page),
            listener: Mutex::new(None),
        }
    }
}

/// The page Studio was started with. Returned once.
#[tauri::command]
pub fn take_initial_page(st: State<'_, NavigationState>) -> Option<String> {
    st.initial.lock().unwrap().take()
}

/// Delivers later page requests (from another launch) to the interface.
#[tauri::command]
pub fn subscribe_navigation(st: State<'_, NavigationState>, channel: Channel<String>) {
    *st.listener.lock().unwrap() = Some(channel);
}

/// Called with the arguments of a second launch.
pub fn on_second_launch<R: Runtime>(app: &AppHandle<R>, args: Vec<String>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    let Some(page) = page_from_args(args.iter().skip(1)) else {
        return;
    };
    if let Some(st) = app.try_state::<NavigationState>() {
        let delivered = st
            .listener
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.send(page.clone()).is_ok())
            .unwrap_or(false);
        if !delivered {
            *st.initial.lock().unwrap() = Some(page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_page_argument_in_both_forms() {
        assert_eq!(
            page_from_args(["--page", "updates"]),
            Some("updates".into())
        );
        assert_eq!(page_from_args(["-x", "--page=apps"]), Some("apps".into()));
        assert_eq!(page_from_args(["--page", "nowhere"]), None);
        assert_eq!(page_from_args(["--page"]), None);
        assert_eq!(page_from_args(Vec::<String>::new()), None);
    }
}
