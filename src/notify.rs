//! Desktop notifications through `org.freedesktop.Notifications`.
//!
//! Notifications are transient, so they stay out of the notification history. They never
//! contain secrets.

use std::collections::HashMap;

use zbus::{Connection, Variant};

const APP_NAME: &str = "Proton Pass Search";

pub async fn send(conn: &Connection, icon: &str, summary: &str, body: &str) {
    let hints = HashMap::from([("transient", Variant::from(true))]);
    let actions: Vec<&str> = Vec::new();
    let expire_default = -1i32;
    let sent = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                APP_NAME,
                0u32,
                icon,
                summary,
                body,
                actions,
                hints,
                expire_default,
            ),
        )
        .await;
    if let Err(e) = sent {
        eprintln!("sending notification failed: {e}");
    }
}
