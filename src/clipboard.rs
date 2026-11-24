//! Copies text to the Wayland clipboard.
//!
//! Two backends:
//! - **data-control:** talks the data-control protocol in-process. KWin and wlroots compositors
//!   offer it; Mutter (GNOME) does not.
//! - **wl-copy:** runs `wl-copy --foreground`, which also works without data-control.
//!
//! Secrets carry the `x-kde-passwordManagerHint: secret` offer, so clipboard managers such as
//! Klipper leave them out of their history, and are cleared after a delay. The clipboard is
//! cleared only if it still holds our copy, so text copied since then survives.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use wl_clipboard_rs::copy::{self, ClipboardType, MimeType, Options, Seat, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Backend {
    /// data-control, falling back to wl-copy when the compositor lacks the protocol.
    Auto,
    DataControl,
    WlCopy,
}

#[derive(Clone, Copy)]
pub enum Content {
    Secret { clear_after: Duration },
    Plain,
}

impl Content {
    fn is_secret(self) -> bool {
        matches!(self, Content::Secret { .. })
    }
}

/// How long `wl-copy` may take to fail before we assume it is serving.
const WL_COPY_STARTUP: Duration = Duration::from_millis(300);

pub async fn copy(backend: Backend, text: &str, content: Content) -> Result<()> {
    match backend {
        Backend::DataControl => copy_data_control(text, content).await,
        Backend::WlCopy => copy_wl_copy(text, content).await,
        Backend::Auto => match copy_data_control(text, content).await {
            Err(e) if is_missing_protocol(&e) => copy_wl_copy(text, content).await,
            other => other,
        },
    }
}

fn is_missing_protocol(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<copy::Error>(),
        Some(copy::Error::MissingProtocol { .. })
    )
}

async fn copy_data_control(text: &str, content: Content) -> Result<()> {
    let bytes: Box<[u8]> = text.as_bytes().into();
    let ours = Arc::new(AtomicBool::new(true));
    let (prepared_tx, prepared_rx) = oneshot::channel();

    // A prepared copy is not `Send`, so it is prepared and served on its own thread. Serving
    // returns once another copy, or our clear, replaces it.
    let serving = ours.clone();
    std::thread::spawn(move || {
        let mut options = Options::new();
        options.foreground(true).sensitive(content.is_secret());
        match options.prepare_copy(Source::Bytes(bytes), MimeType::Text) {
            Ok(prepared) => {
                let _ = prepared_tx.send(Ok(()));
                if let Err(e) = prepared.serve() {
                    eprintln!("serving clipboard failed: {e}");
                }
            }
            Err(e) => {
                let _ = prepared_tx.send(Err(e));
            }
        }
        serving.store(false, Ordering::SeqCst);
    });
    prepared_rx
        .await
        .context("clipboard thread ended early")??;

    if let Content::Secret { clear_after } = content {
        tokio::spawn(async move {
            tokio::time::sleep(clear_after).await;
            if ours.load(Ordering::SeqCst) {
                let cleared =
                    tokio::task::spawn_blocking(|| copy::clear(ClipboardType::Regular, Seat::All))
                        .await;
                if let Ok(Err(e)) = cleared {
                    eprintln!("clearing clipboard failed: {e}");
                }
            }
        });
    }
    Ok(())
}

/// `wl-copy --foreground` exits once another copy replaces ours, so a running child means the
/// clipboard still holds our text.
async fn copy_wl_copy(text: &str, content: Content) -> Result<()> {
    let mut command = Command::new("wl-copy");
    command.args(["--foreground", "--type", "text/plain"]);
    if content.is_secret() {
        command.arg("--sensitive");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting `wl-copy`")?;
    let mut stdin = child.stdin.take().context("`wl-copy` has no stdin")?;
    stdin
        .write_all(text.as_bytes())
        .await
        .context("writing to `wl-copy`")?;
    drop(stdin);

    if let Ok(status) = tokio::time::timeout(WL_COPY_STARTUP, child.wait()).await {
        let status = status.context("waiting for `wl-copy`")?;
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = pipe.read_to_string(&mut stderr).await;
        }
        bail!("`wl-copy` exited at once ({status}): {}", stderr.trim());
    }

    tokio::spawn(async move {
        if let Content::Secret { clear_after } = content {
            clear_if_still_serving(&mut child, clear_after).await;
        }
        let _ = child.wait().await;
    });
    Ok(())
}

async fn clear_if_still_serving(child: &mut Child, clear_after: Duration) {
    if tokio::time::timeout(clear_after, child.wait())
        .await
        .is_ok()
    {
        return;
    }
    let cleared = Command::new("wl-copy").arg("--clear").status().await;
    if !matches!(cleared, Ok(status) if status.success()) {
        eprintln!("`wl-copy --clear` failed: {cleared:?}");
    }
}
