//! What happens when the user picks a result: copy one field of the item, then notify.

use std::time::Duration;

use anyhow::{Result, bail};
use zbus::Connection;

use crate::clipboard::{self, Backend, Content};
use crate::index::Entry;
use crate::notify;
use crate::pass_cli;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Password,
    /// The username, or the email when the item has no username.
    Username,
    /// The current one-time code.
    Totp,
}

impl Field {
    pub fn describe(self) -> &'static str {
        match self {
            Field::Password => "password",
            Field::Username => "username",
            Field::Totp => "one-time code",
        }
    }

    /// Whether `entry` has this field, judged from the listing.
    pub fn available(self, entry: &Entry) -> bool {
        match self {
            Field::Password => true,
            Field::Username => !entry.account.is_empty(),
            Field::Totp => entry.login.has_totp,
        }
    }
}

#[derive(Clone, Copy)]
pub struct CopySettings {
    pub backend: Backend,
    pub clear_after: Duration,
}

/// Copies `field` of `entry` in the background and reports the outcome as a notification.
pub fn copy_and_notify(conn: &Connection, settings: CopySettings, entry: &Entry, field: Field) {
    let conn = conn.clone();
    let title = entry.login.title.clone();
    let share_id = entry.login.share_id.clone();
    let item_id = entry.login.item_id.clone();
    let account = entry.account.clone();
    tokio::spawn(async move {
        let copied = copy(settings, &share_id, &item_id, &account, field).await;
        let (icon, summary, body) = match copied {
            Ok(what) => ("dialog-password", format!("Copied {what}"), title),
            Err(e) => (
                "dialog-error",
                format!("Could not copy from {title}"),
                format!("{e:#}"),
            ),
        };
        notify::send(&conn, icon, &summary, &body).await;
    });
}

/// Returns what was copied, for the notification.
async fn copy(
    settings: CopySettings,
    share_id: &str,
    item_id: &str,
    account: &str,
    field: Field,
) -> Result<String> {
    let secret = Content::Secret {
        clear_after: settings.clear_after,
    };
    let clears = format!("clears in {} s", settings.clear_after.as_secs());
    match field {
        Field::Password => {
            let password = pass_cli::password(share_id, item_id).await?;
            if password.is_empty() {
                bail!("the item has no password");
            }
            clipboard::copy(settings.backend, &password, secret).await?;
            Ok(format!("password ({clears})"))
        }
        Field::Username => {
            if account.is_empty() {
                bail!("the item has no username or email");
            }
            clipboard::copy(settings.backend, account, Content::Plain).await?;
            Ok("username".to_owned())
        }
        Field::Totp => {
            let code = pass_cli::totp(share_id, item_id).await?;
            clipboard::copy(settings.backend, &code, secret).await?;
            Ok(format!("one-time code ({clears})"))
        }
    }
}
