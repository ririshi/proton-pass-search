//! The KRunner interface, end to end against a fake `pass-cli`.

mod common;

use std::time::Duration;

use common::{Options, RemoteMatch, Session};

const GITHUB: &str = "share-personal/item-github";
const BANK: &str = "share-personal/item-bank";
const SSO: &str = "share-work/item-sso";
const HINT: &str = "x-kde-passwordManagerHint";

fn ids(matches: &[RemoteMatch]) -> Vec<&str> {
    matches.iter().map(|m| m.0.as_str()).collect()
}

fn prop(m: &RemoteMatch, key: &str) -> String {
    String::try_from(m.5[key].try_clone().unwrap()).unwrap()
}

fn actions(m: &RemoteMatch) -> Vec<String> {
    Vec::<String>::try_from(m.5["actions"].try_clone().unwrap()).unwrap()
}

async fn session() -> Session {
    Session::start(Options::default()).await
}

async fn session_with(args: &[&str], wayland: bool) -> Session {
    let args = args.iter().map(|s| s.to_string()).collect();
    Session::start(Options {
        wayland,
        args,
        fail_with: None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn config_restricts_queries_to_prefix() {
    let s = session().await;
    let config = s.krunner_config().await;
    assert_eq!(
        String::try_from(config["MatchRegex"].try_clone().unwrap()).unwrap(),
        "^pp "
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn actions_are_username_and_totp() {
    let s = session().await;
    let ids: Vec<_> = s.krunner_actions().await.into_iter().map(|a| a.0).collect();
    assert_eq!(ids, ["username", "totp"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn title_match_ranks_above_url_match() {
    let s = session().await;
    let matches = s.krunner_match("pp github").await;
    assert_eq!(ids(&matches), [GITHUB, SSO]);
    assert_eq!(matches[0].1, "GitHub");
    assert_eq!(matches[0].4, 1.0);
    assert!(matches[1].4 < 1.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn equal_scores_keep_title_order_with_decreasing_relevance() {
    let s = session().await;
    // Both have the email alice@example.com; the SSO item only a URL on example.com.
    let matches = s.krunner_match("pp example").await;
    assert_eq!(ids(&matches), [BANK, GITHUB, SSO]);
    assert!(matches.windows(2).all(|w| w[0].4 > w[1].4));
}

#[tokio::test(flavor = "multi_thread")]
async fn trashed_items_and_other_types_are_left_out() {
    let s = session().await;
    let titles: Vec<_> = s
        .krunner_match("pp github")
        .await
        .into_iter()
        .map(|m| m.1)
        .collect();
    assert!(!titles.contains(&"Old GitHub".to_owned()));
    assert!(!titles.contains(&"GitHub notes".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn single_runner_mode_needs_no_prefix() {
    let s = session().await;
    assert_eq!(ids(&s.krunner_match("github").await), [GITHUB, SSO]);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_word_must_match() {
    let s = session().await;
    assert_eq!(ids(&s.krunner_match("pp github bob").await), [SSO]);
}

#[tokio::test(flavor = "multi_thread")]
async fn matching_ignores_accents_and_case() {
    let s = session().await;
    assert_eq!(ids(&s.krunner_match("pp BANK").await), [BANK]);
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_and_unmatched_queries_return_nothing() {
    let s = session().await;
    assert!(s.krunner_match("pp ").await.is_empty());
    assert!(s.krunner_match("pp    ").await.is_empty());
    assert!(s.krunner_match("pp nothing-like-this").await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn results_show_account_host_vault_and_available_actions() {
    let s = session().await;
    let github = &s.krunner_match("pp github").await[0];
    assert_eq!(prop(github, "subtext"), "alice · github.com · Personal");
    assert_eq!(prop(github, "category"), "Proton Pass");
    assert_eq!(actions(github), ["username", "totp"]);

    let bank = &s.krunner_match("pp bank").await[0];
    assert_eq!(prop(bank, "subtext"), "alice@example.com · Personal");
    assert_eq!(actions(bank), ["username"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_vault_is_listed_once_with_filters() {
    let s = session().await;
    s.krunner_match("pp github").await;
    s.krunner_match("pp bank").await;
    let mut calls = s.pass_cli_calls();
    calls.sort();
    let list = |share: &str| {
        format!(
            "item list --share-id {share} --filter-state active --filter-type login --output json --show-secrets"
        )
    };
    assert_eq!(
        calls,
        [
            list("share-personal"),
            list("share-work"),
            "vault list --output json".to_owned()
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn load_error_shows_as_a_result_and_running_it_does_nothing() {
    let options = Options {
        fail_with: Some("Error: the session is locked"),
        ..Options::default()
    };
    let s = Session::start(options).await;
    let matches = s.krunner_match("pp github").await;
    assert_eq!(ids(&matches), ["error"]);
    assert!(prop(&matches[0], "subtext").contains("the session is locked"));
    assert!(actions(&matches[0]).is_empty());

    s.krunner_run("error", "").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.notification_count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn teardown_reloads_stale_data() {
    let s = session_with(&["--refresh-after", "0"], false).await;
    assert!(s.krunner_match("pp renamed").await.is_empty());
    let work = s.data.join("items-share-work.json");
    let json = std::fs::read_to_string(&work)
        .unwrap()
        .replace("Work SSO", "Work SSO renamed");
    std::fs::write(&work, json).unwrap();

    s.krunner_teardown().await;
    let mut found = Vec::new();
    for _ in 0..100 {
        found = s.krunner_match("pp renamed").await;
        if !found.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(ids(&found), [SSO]);
}

#[tokio::test(flavor = "multi_thread")]
async fn teardown_keeps_fresh_data() {
    let s = session().await;
    s.krunner_match("pp github").await;
    let before = s.pass_cli_calls().len();
    s.krunner_teardown().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.pass_cli_calls().len(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_reload_keeps_earlier_results() {
    let s = session_with(&["--refresh-after", "0"], false).await;
    assert_eq!(ids(&s.krunner_match("pp github").await), [GITHUB, SSO]);
    std::fs::write(s.data.join("fail"), "Error: network down").unwrap();
    let before = s.pass_cli_calls().len();
    s.krunner_teardown().await;
    s.eventually(Duration::from_secs(5), || {
        (s.pass_cli_calls().len() > before).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(ids(&s.krunner_match("pp github").await), [GITHUB, SSO]);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_match_id_does_nothing() {
    let s = session().await;
    s.krunner_match("pp github").await;
    s.krunner_run("share-x/item-x", "").await;
    s.krunner_run(GITHUB, "no-such-action").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.notification_count(), 0);
}

/// Clipboard tests, run once per clipboard backend.
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
    password_is_copied_marked_secret_and_cleared,
    text_copied_since_is_not_cleared,
    username_is_copied_plain_and_kept,
    username_falls_back_to_email,
    one_time_code_is_copied_and_cleared,
    pass_cli_failure_is_reported,
);

async fn clipboard_session(backend: &str) -> Session {
    let s = session_with(&["--clear-after", "1", "--clipboard", backend], true).await;
    s.krunner_match("pp github").await;
    s
}

async fn password_is_copied_marked_secret_and_cleared(backend: &str) {
    let s = clipboard_session(backend).await;
    s.krunner_run(GITHUB, "").await;
    let n = s.next_notification().await;
    assert_eq!(
        (n.summary.as_str(), n.body.as_str()),
        ("Copied password (clears in 1 s)", "GitHub")
    );
    assert!(n.transient);
    assert_eq!(s.paste().as_deref(), Some("gh-secret-pass"));
    assert!(s.paste_types().contains(&HINT.to_owned()));
    s.eventually(Duration::from_secs(5), || s.paste().is_none().then_some(()))
        .await;
}

async fn text_copied_since_is_not_cleared(backend: &str) {
    let s = clipboard_session(backend).await;
    s.krunner_run(GITHUB, "").await;
    s.next_notification().await;
    s.copy_as_other_app("copied later");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(s.paste().as_deref(), Some("copied later"));
}

async fn username_is_copied_plain_and_kept(backend: &str) {
    let s = clipboard_session(backend).await;
    s.krunner_run(GITHUB, "username").await;
    assert_eq!(s.next_notification().await.summary, "Copied username");
    assert_eq!(s.paste().as_deref(), Some("alice"));
    assert!(!s.paste_types().contains(&HINT.to_owned()));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(s.paste().as_deref(), Some("alice"));
}

async fn username_falls_back_to_email(backend: &str) {
    let s = clipboard_session(backend).await;
    s.krunner_run(BANK, "username").await;
    s.next_notification().await;
    assert_eq!(s.paste().as_deref(), Some("alice@example.com"));
}

async fn one_time_code_is_copied_and_cleared(backend: &str) {
    let s = clipboard_session(backend).await;
    s.krunner_run(GITHUB, "totp").await;
    assert_eq!(
        s.next_notification().await.summary,
        "Copied one-time code (clears in 1 s)"
    );
    assert_eq!(s.paste().as_deref(), Some("123456"));
    assert!(s.paste_types().contains(&HINT.to_owned()));
    s.eventually(Duration::from_secs(5), || s.paste().is_none().then_some(()))
        .await;
}

async fn pass_cli_failure_is_reported(backend: &str) {
    let s = clipboard_session(backend).await;
    std::fs::write(s.data.join("fail-view"), "Error: the session is locked").unwrap();
    s.krunner_run(GITHUB, "").await;
    let n = s.next_notification().await;
    assert_eq!(n.summary, "Could not copy from GitHub");
    assert!(n.body.contains("the session is locked"), "{}", n.body);
    assert_eq!(s.paste(), None);
}
