//! Native menu bar, built from the web UI's own action registry
//! (issues #817 / #818).
//!
//! # Why the page decides what is in the menu
//!
//! The alternative -- a hardcoded menu here in Rust -- means a second
//! copy of `web/src/library/actions.ts`, which drifts the first time
//! an action is added on the web side, and which would happily offer
//! menu entries for actions the page has no handler for. Instead the
//! page sends a spec of exactly what it can currently do (see
//! `web/src/library/desktopMenu.ts`) and this module renders it.
//!
//! # Why menu clicks come back as a DOM `CustomEvent`
//!
//! `web/` deliberately carries no `@tauri-apps/api` dependency: it is
//! a plain browser SPA that this app navigates its webview to, and it
//! must keep working when served as an ordinary web page. Rather than
//! reimplement Tauri's event-listener protocol against
//! `__TAURI_INTERNALS__.transformCallback`, a menu click evaluates a
//! one-line script in the webview that dispatches a `CustomEvent`.
//! Ordinary DOM on the receiving end, and no npm dependency added to a
//! package that also has to run as a plain browser tab.

use serde::Deserialize;
use tauri::menu::{Menu, MenuEvent, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime, WebviewWindow};

use crate::page_event;

/// The `CustomEvent` name dispatched into the page. Must match
/// `MENU_ACTION_EVENT` in `web/src/library/desktopMenu.ts`.
const MENU_ACTION_EVENT: &str = "oxide:menu-action";

/// One action as described by the page. Mirrors `MenuActionSpec` in
/// `web/src/library/actions.ts`.
#[derive(Debug, Clone, Deserialize)]
pub struct MenuActionSpec {
    pub id: String,
    pub label: String,
    pub group: String,
    pub enabled: bool,
}

/// Human-readable submenu title for a registry group. An unknown
/// group gets its own submenu rather than being dropped, so adding a
/// group on the web side degrades to "shows up under a raw name"
/// instead of "silently disappears from the menu".
fn group_title(group: &str) -> String {
    match group {
        "library" => "Library".to_string(),
        "selection" => "Selection".to_string(),
        "book" => "Book".to_string(),
        "view" => "View".to_string(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => "Other".to_string(),
            }
        }
    }
}

/// Group order in the menu bar. Groups not named here follow, in the
/// order the page listed them.
const GROUP_ORDER: &[&str] = &["library", "selection", "book", "view"];

/// Builds and installs the menu for `window` from `actions`.
///
/// Rebuilding wholesale on every change is deliberate: Tauri's menu
/// items are immutable once built on some platforms, and the spec is
/// a handful of entries, so a rebuild is cheaper than tracking
/// per-item diffs and getting enablement subtly wrong.
pub fn install<R: Runtime>(window: &WebviewWindow<R>, actions: &[MenuActionSpec]) -> tauri::Result<()> {
    let handle = window.app_handle();

    // File: app-level items the page has no say over.
    let file = Submenu::with_items(handle, "File", true, &[&PredefinedMenuItem::close_window(handle, Some("Close Window"))?, &PredefinedMenuItem::separator(handle)?, &PredefinedMenuItem::quit(handle, Some("Quit"))?])?;

    let mut submenus: Vec<Submenu<R>> = vec![file];

    let mut groups: Vec<&str> = Vec::new();
    for g in GROUP_ORDER {
        if actions.iter().any(|a| a.group == *g) {
            groups.push(g);
        }
    }
    for a in actions {
        if !groups.contains(&a.group.as_str()) {
            groups.push(&a.group);
        }
    }

    for group in groups {
        let items = actions
            .iter()
            .filter(|a| a.group == group)
            .map(|a| MenuItemBuilder::with_id(a.id.clone(), &a.label).enabled(a.enabled).build(handle))
            .collect::<tauri::Result<Vec<_>>>()?;
        let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<R>).collect();
        submenus.push(Submenu::with_items(handle, group_title(group), true, &refs)?);
    }

    // Edit: the standard clipboard items. Without these, the OS-level
    // copy/paste shortcuts do not work in text inputs on macOS.
    let edit = Submenu::with_items(
        handle,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(handle, None)?,
            &PredefinedMenuItem::redo(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::cut(handle, None)?,
            &PredefinedMenuItem::copy(handle, None)?,
            &PredefinedMenuItem::paste(handle, None)?,
            &PredefinedMenuItem::select_all(handle, None)?,
        ],
    )?;
    submenus.insert(1, edit);

    let refs: Vec<&dyn tauri::menu::IsMenuItem<R>> = submenus.iter().map(|s| s as &dyn tauri::menu::IsMenuItem<R>).collect();
    let menu = Menu::with_items(handle, &refs)?;
    window.set_menu(menu)?;
    Ok(())
}

/// Forwards a menu activation into the page as a `CustomEvent`.
pub fn forward<R: Runtime>(app: &AppHandle<R>, event: &MenuEvent) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    page_event::dispatch(&window, MENU_ACTION_EVENT, &serde_json::json!({ "id": event.id().0.as_str() }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_groups_get_readable_titles() {
        assert_eq!(group_title("library"), "Library");
        assert_eq!(group_title("book"), "Book");
    }

    #[test]
    fn an_unknown_group_is_capitalized_rather_than_dropped() {
        // A group added on the web side should degrade to a visible
        // raw name, not vanish from the menu bar without a trace.
        assert_eq!(group_title("annotations"), "Annotations");
    }

    #[test]
    fn an_empty_group_name_does_not_panic() {
        assert_eq!(group_title(""), "Other");
    }

}
