//! `cmux setup notifications` driven the way an agent drives it, against a
//! fake cmux app whose person answers the macOS notifications prompt.

use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::{World, serve_app};

/// System Settings > Notifications, on cmux's own page.
const PANE: &str =
    "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.cmuxterm.app";

/// A fake cmux app on a socket of the world's: `notifications.status`
/// answers `permission`, and `notifications.request` is the macOS prompt:
/// while not determined it turns authorized when `allow` is set, denied
/// otherwise. With `old`, it is an app from before these methods and
/// answers every request `method_not_found`. Every request is kept.
struct NotificationsApp {
    permission: Arc<Mutex<Value>>,
    allow: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl NotificationsApp {
    fn start(world: &mut World, old: bool) -> Self {
        let socket = world.home.parent().unwrap().join("app.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        world.app = Some(socket);
        let app = Self {
            permission: Arc::new(Mutex::new(json!({"authorization": "not_determined",
                "alert": false, "sound": false, "badge": false, "bundle_id": "com.cmuxterm.app"}))),
            allow: Arc::new(AtomicBool::new(true)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let (permission, allow) = (app.permission.clone(), app.allow.clone());
        serve_app(listener, app.requests.clone(), move |method, _| {
            Some(match method {
                "notifications.status" if !old => Ok(permission.lock().unwrap().clone()),
                "notifications.request" if !old => {
                    let mut current = permission.lock().unwrap();
                    if current["authorization"] == "not_determined" {
                        let allowed = allow.load(Ordering::SeqCst);
                        current["authorization"] =
                            json!(if allowed { "authorized" } else { "denied" });
                        for key in ["alert", "sound", "badge"] {
                            current[key] = json!(allowed);
                        }
                    }
                    Ok(current.clone())
                }
                _ => Err(json!({"code": "method_not_found", "message": "unknown method"})),
            })
        });
        app
    }

    /// How many requests for `method` the app received.
    fn calls(&self, method: &str) -> usize {
        self.requests.lock().unwrap().iter().filter(|r| r["method"] == method).count()
    }
}

/// Notifications driven the way an agent drives them: `status` names the
/// task, a dry run asks nothing, the macOS prompt asks once and the person
/// allows it, a rerun asks nothing, and the undo is the pane where the person
/// turns them off. Denied (the prompt is spent), setup names the System
/// Settings pane to open; declined on the prompt, the agent asks first.
#[test]
fn cmux_setup_notifications_asks_macos_once_and_names_the_pane_when_denied() {
    let mut world = World::new("nt");
    let app = NotificationsApp::start(&mut world, false);

    let status = world.json(&["setup", "status"]);
    let check = &status["checks"][6];
    assert_eq!(check["task"], "notifications", "{status}");
    assert_eq!(check["state"], "needs_setup", "{check}");
    let fix = check["fix"].as_str().expect("status names its fix").to_owned();
    assert_eq!(fix, "cmux setup notifications");

    let planned = world.json(&["setup", "notifications", "--dry-run"]);
    assert_eq!(planned["state"], "planned", "{planned}");
    assert_eq!(planned["undo"], format!("open '{PANE}'"), "{planned}");
    assert_eq!(app.calls("notifications.request"), 0, "a dry run showed the prompt");

    let allowed = world.follow(&fix);
    assert_eq!(allowed["state"], "changed", "{allowed}");
    assert_eq!(allowed["before"]["authorization"], "not_determined");
    assert_eq!(allowed["after"]["authorization"], "authorized");
    assert_eq!(allowed["undo"], format!("open '{PANE}'"), "{allowed}");
    assert!(allowed["undo_note"].is_string(), "{allowed}");
    assert_eq!(app.calls("notifications.request"), 1);

    let rerun = world.json(&["setup", "notifications"]);
    assert_eq!(rerun["state"], "ok", "{rerun}");
    assert_eq!(rerun["undo"], Value::Null);
    assert_eq!(app.calls("notifications.request"), 1, "an ok run showed the prompt");
    assert_eq!(world.json(&["setup", "status"])["checks"][6]["state"], "ok");

    // Turned off in System Settings: setup names the pane, and asks nothing.
    app.permission.lock().unwrap()["authorization"] = json!("denied");
    let denied = world.json(&["setup", "notifications"]);
    assert_eq!(denied["state"], "needs_user", "{denied}");
    let action = &denied["user_action"];
    assert_eq!(action["kind"], "grant", "{action}");
    assert_eq!(action["command"], format!("open '{PANE}'"));
    assert_eq!(action["url"], PANE);
    assert_eq!(app.calls("notifications.request"), 1, "a denied run showed the prompt");
    assert_eq!(world.json(&["setup", "status"])["checks"][6]["state"], "needs_user");

    // The person says no on the prompt: nothing to undo, ask before again.
    let mut world = World::new("ntno");
    let app = NotificationsApp::start(&mut world, false);
    app.allow.store(false, Ordering::SeqCst);
    let declined = world.json(&["setup", "notifications"]);
    assert_eq!(declined["state"], "needs_user", "{declined}");
    assert_eq!(declined["user_action"]["kind"], "declined", "{declined}");
    assert_eq!(declined["undo"], Value::Null);

    let output = world.run(&["setup", "notifications", "now"]);
    assert_eq!(output.status.code(), Some(2));
}

/// An app from before `notifications.status`: the person gets the pane.
#[test]
fn cmux_setup_notifications_names_an_older_app() {
    let mut world = World::new("ntold");
    let _app = NotificationsApp::start(&mut world, true);
    let result = world.json(&["setup", "notifications"]);
    assert_eq!(result["state"], "needs_user", "{result}");
    assert_eq!(result["user_action"]["kind"], "app", "{result}");
    assert_eq!(
        result["user_action"]["command"],
        "open 'x-apple.systempreferences:com.apple.Notifications-Settings.extension'",
        "{result}"
    );
}
