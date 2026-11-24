//! Searches Proton Pass logins from KDE's KRunner and GNOME Shell's overview search, through
//! Proton's `pass-cli`.
//!
//! The process is a D-Bus service that the desktop starts on demand; see `install.sh`. It
//! serves both desktops' interfaces at once, each at its own object path.

mod actions;
mod clipboard;
mod gnome;
mod index;
mod krunner;
mod notify;
mod pass_cli;
mod search;

use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;

use crate::actions::CopySettings;

const BUS_NAME: &str = "local.ProtonPassSearch";
const KRUNNER_PATH: &str = "/krunner";
const GNOME_PATH: &str = "/gnome";

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Seconds before a copied password or one-time code is cleared from the clipboard.
    #[arg(long, default_value_t = 30)]
    clear_after: u64,
    /// Seconds after which the item list is reloaded on the next search.
    #[arg(long, default_value_t = 300)]
    refresh_after: u64,
    /// How to reach the clipboard.
    #[arg(long, value_enum, default_value_t = clipboard::Backend::Auto)]
    clipboard: clipboard::Backend,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let copy = CopySettings {
        backend: args.clipboard,
        clear_after: Duration::from_secs(args.clear_after),
    };
    let refresh_after = Duration::from_secs(args.refresh_after);

    let index = Arc::new(index::Index::new());
    tokio::spawn({
        let index = index.clone();
        async move { index.refresh().await }
    });
    let _conn = zbus::connection::Builder::session()
        .name(BUS_NAME)
        .serve_at(
            KRUNNER_PATH,
            krunner::Runner::new(index.clone(), copy, refresh_after),
        )
        .serve_at(
            GNOME_PATH,
            gnome::SearchProvider::new(index, copy, refresh_after),
        )
        .build()
        .await
        .with_context(|| format!("claiming {BUS_NAME} on the session bus"))?;
    pending::<()>().await;
    Ok(())
}
