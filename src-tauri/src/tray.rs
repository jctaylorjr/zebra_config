use crate::{AppState, model::Status};
use std::sync::atomic::Ordering;
use tauri::{
    AppHandle, Manager, Window, WindowEvent,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_notification::NotificationExt;

const TRAY: &str = "main";

// how the printers on the Configure page are doing
#[derive(Debug, Default, PartialEq)]
struct Work {
    busy: usize, // applying, or waiting to retry an offline printer
    applied: usize,
    failed: usize,
}

fn work(app: &AppHandle) -> Work {
    let state = app.state::<AppState>();
    let printers = state.printers.lock().unwrap();
    tally(printers.values().map(|p| &p.status))
}

fn tally<'a>(statuses: impl Iterator<Item = &'a Status>) -> Work {
    let mut w = Work::default();
    for s in statuses {
        match s {
            Status::Applying(_) | Status::Offline { .. } => w.busy += 1,
            Status::Applied(_) => w.applied += 1,
            Status::Failed(_) | Status::Unverified(_) => w.failed += 1,
            Status::Pending => {}
        }
    }
    w
}

fn printers(n: usize) -> String {
    format!("{n} {}", if n == 1 { "printer" } else { "printers" })
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "Open Zebra Configure", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &quit_item])?;
    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("Zebra Configure")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show(app),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

pub fn show(app: &AppHandle) {
    // opened again, so closing the window works as usual again
    app.state::<AppState>()
        .closed_while_busy
        .store(false, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// quitting partway through cuts off whatever is being sent, even firmware, so it asks first
fn quit(app: &AppHandle) {
    let w = work(app);
    if w.busy == 0 {
        return app.exit(0);
    }
    let app = app.clone();
    app.dialog()
        .message(format!(
            "{} still being configured. Quitting now stops them partway.",
            printers(w.busy)
        ))
        .title("Quit Zebra Configure?")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit".into(),
            "Keep running".into(),
        ))
        .show(move |quit| {
            if quit {
                app.exit(0);
            }
        });
}

// closing the window while printers are being configured keeps the app in the tray until
// they're done, instead of stopping them
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let app = window.app_handle();
    let w = work(app);
    if w.busy == 0 {
        return; // closes, and the app quits as usual
    }
    api.prevent_close();
    let _ = window.hide();
    app.state::<AppState>()
        .closed_while_busy
        .store(true, Ordering::SeqCst);
    notify(
        app,
        &format!(
            "Still configuring {}. Zebra Configure stays in the notification area until they're done.",
            printers(w.busy)
        ),
    );
}

// after every printer status change: the tooltip follows along, and once a window that was
// closed mid-job has nothing left to do, the app finishes up
pub fn update(app: &AppHandle) {
    let w = work(app);
    if let Some(tray) = app.tray_by_id(TRAY) {
        let tip = match w.busy {
            0 => "Zebra Configure".to_string(),
            n => format!("Zebra Configure: configuring {}", printers(n)),
        };
        let _ = tray.set_tooltip(Some(tip));
    }
    let state = app.state::<AppState>();
    if w.busy > 0 || !state.closed_while_busy.load(Ordering::SeqCst) {
        return;
    }
    state.closed_while_busy.store(false, Ordering::SeqCst);
    if w.failed == 0 {
        notify(
            app,
            &format!("Finished configuring: {} applied.", printers(w.applied)),
        );
        app.exit(0);
    } else {
        // stays in the tray, so the failures can still be looked at
        notify(
            app,
            &format!(
                "Finished: {} applied, {} need attention. Open Zebra Configure from the notification area to see why.",
                w.applied, w.failed
            ),
        );
    }
}

fn notify(app: &AppHandle, body: &str) {
    let _ = app
        .notification()
        .builder()
        .title("Zebra Configure")
        .body(body)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_printers_still_count_as_busy() {
        let statuses = [
            Status::Applying("Sending files".into()),
            Status::Offline {
                attempt: 2,
                retry_in_secs: 10,
                reason: "timed out".into(),
            },
            Status::Applied("4 settings".into()),
            Status::Failed("did not read back".into()),
            Status::Unverified("restarted".into()),
            Status::Pending,
        ];
        assert_eq!(
            tally(statuses.iter()),
            Work {
                busy: 2,
                applied: 1,
                failed: 2
            }
        );
        assert_eq!(printers(1), "1 printer");
        assert_eq!(printers(3), "3 printers");
    }
}
