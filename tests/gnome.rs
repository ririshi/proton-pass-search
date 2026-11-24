//! The GNOME Shell search provider interface, end to end against a fake `pass-cli`.
//!
//! The clipboard runs on a headless KWin here. On GNOME itself the `auto` backend falls back to
//! `wl-copy`, because Mutter lacks the data-control protocol; both backends are tested.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::{Options, Session};
use zbus::OwnedVariant;

const GITHUB: &str = "share-personal/item-github";
const SSO: &str = "share-work/item-sso";

fn meta(m: &HashMap<String, OwnedVariant>, key: &str) -> String {
    String::try_from(m[key].try_clone().unwrap()).unwrap()
}

async fn session(wayland: bool, backend: &str) -> Session {
    let args = ["--clear-after", "1", "--clipboard", backend]
        .map(str::to_owned)
        .to_vec();
    Session::start(Options {
        wayland,
        args,
        fail_with: None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn searches_without_prefix_return_nothing() {
    let s = session(false, "auto").await;
    assert!(s.gnome_initial("github").await.is_empty());
    assert!(s.gnome_initial("pp").await.is_empty());
    assert!(
        s.pass_cli_calls()
            .iter()
            .all(|c| !c.starts_with("item view"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prefix_selects_password_results() {
    let s = session(false, "auto").await;
    let ids = s.gnome_initial("pp github").await;
    assert_eq!(ids, [format!("p:{GITHUB}"), format!("p:{SSO}")]);
}

#[tokio::test(flavor = "multi_thread")]
async fn mode_word_selects_the_field() {
    let s = session(false, "auto").await;
    assert_eq!(
        s.gnome_initial("pp u github").await,
        [format!("u:{GITHUB}"), format!("u:{SSO}")]
    );
    // Only GitHub has a one-time code.
    assert_eq!(
        s.gnome_initial("pp t github").await,
        [format!("t:{GITHUB}")]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn subsearch_searches_again() {
    let s = session(false, "auto").await;
    let first = s.gnome_initial("pp git").await;
    assert_eq!(
        s.gnome_subsearch(&first, "pp github bob").await,
        [format!("p:{SSO}")]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn metas_describe_what_gets_copied() {
    let s = session(false, "auto").await;
    let ids = vec![
        format!("p:{GITHUB}"),
        format!("u:{SSO}"),
        "p:share-x/item-x".to_owned(),
    ];
    let metas = s.gnome_metas(&ids).await;
    assert_eq!(metas.len(), 2, "unknown ids are left out");
    assert_eq!(meta(&metas[0], "id"), ids[0]);
    assert_eq!(meta(&metas[0], "name"), "GitHub");
    assert_eq!(
        meta(&metas[0], "description"),
        "Copy password · alice · github.com · Personal"
    );
    assert_eq!(
        meta(&metas[0], "gicon"),
        ". GThemedIcon dialog-password dialog-password-symbolic"
    );
    assert_eq!(
        meta(&metas[1], "description"),
        "Copy username · bob · github.com · Work"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_ids_do_nothing() {
    let s = session(false, "auto").await;
    s.gnome_initial("pp github").await;
    s.gnome_activate("nonsense").await;
    s.gnome_activate("x:share-personal/item-github").await;
    s.gnome_activate("p:share-x/item-x").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.notification_count(), 0);
}

macro_rules! per_backend {
    ($($name:ident),* $(,)?) => {
        $(
            mod $name {
                #[tokio::test(flavor = "multi_thread")]
                async fn data_control() {
                    super::$name("data-control").await
                }
                #[tokio::test(flavor = "multi_thread")]
                async fn wl_copy() {
                    super::$name("wl-copy").await
                }
            }
        )*
    };
}

per_backend!(
    activating_copies_password,
    activating_copies_username,
    activating_copies_code
);

async fn activate(backend: &str, terms: &str) -> Session {
    let s = session(true, backend).await;
    let ids = s.gnome_initial(terms).await;
    s.gnome_activate(&ids[0]).await;
    s
}

async fn activating_copies_password(backend: &str) {
    let s = activate(backend, "pp github").await;
    assert_eq!(
        s.next_notification().await.summary,
        "Copied password (clears in 1 s)"
    );
    assert_eq!(s.paste().as_deref(), Some("gh-secret-pass"));
    s.eventually(Duration::from_secs(5), || s.paste().is_none().then_some(()))
        .await;
}

async fn activating_copies_username(backend: &str) {
    let s = activate(backend, "pp u github").await;
    assert_eq!(s.next_notification().await.summary, "Copied username");
    assert_eq!(s.paste().as_deref(), Some("alice"));
}

async fn activating_copies_code(backend: &str) {
    let s = activate(backend, "pp t github").await;
    assert_eq!(
        s.next_notification().await.summary,
        "Copied one-time code (clears in 1 s)"
    );
    assert_eq!(s.paste().as_deref(), Some("123456"));
}
