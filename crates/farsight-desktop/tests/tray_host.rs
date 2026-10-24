//! M6 council condition: the viewer tray reports `NoHost` without a
//! `StatusNotifierItem` host and registers with one, tested on a private
//! session bus. Run as
//! `dbus-run-session -- env FARSIGHT_TRAY_TEST=1 cargo test -p farsight-desktop --test tray_host`
//! (skipped otherwise, so it never touches the developer's real bus).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// zbus interface methods must take `&self` and keep their D-Bus argument names.
#![allow(clippy::unused_self, clippy::used_underscore_binding)]
#![cfg(target_os = "linux")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use farsight_desktop::tray::{TrayAvailability, TrayModel, ViewerTray};

/// A minimal `org.kde.StatusNotifierWatcher` that records registrations.
struct StubWatcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl StubWatcher {
    fn register_status_notifier_item(&self, service: &str) {
        self.items.lock().unwrap().push(service.to_owned());
    }

    fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

#[tokio::test]
async fn tray_without_and_with_a_host() {
    if std::env::var("FARSIGHT_TRAY_TEST").as_deref() != Ok("1") {
        eprintln!("skipped: run under dbus-run-session with FARSIGHT_TRAY_TEST=1");
        return;
    }
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    // No watcher on the bus: an explicit NoHost, no error, no panic.
    let (_tray, availability) = ViewerTray::start(TrayModel::default(), tx.clone()).await;
    assert_eq!(availability, TrayAvailability::NoHost);

    // A watcher appears: the tray registers with it.
    let items = Arc::new(Mutex::new(Vec::new()));
    let _bus = zbus::connection::Builder::session()
        .unwrap()
        .name("org.kde.StatusNotifierWatcher")
        .unwrap()
        .serve_at(
            "/StatusNotifierWatcher",
            StubWatcher {
                items: Arc::clone(&items),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let (_tray, availability) = ViewerTray::start(TrayModel::default(), tx).await;
    assert_eq!(availability, TrayAvailability::Available);
    for _ in 0..50 {
        if !items.lock().unwrap().is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the tray never registered with the watcher");
}
