//! In-memory list of login items, loaded from all vaults in parallel.
//!
//! Entries hold titles, usernames, emails and URLs, never secrets. Nothing is written to disk.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinSet;

use crate::pass_cli::{self, Login};
use crate::search::{Field, Query, Weight};

pub struct Entry {
    pub login: Login,
    pub vault: String,
    /// The username, or the email when the item has no username.
    pub account: String,
    /// Host name of the first URL, if any.
    pub host: String,
    fields: Vec<Field>,
}

impl Entry {
    fn new(login: Login, vault: String) -> Self {
        let account = if login.username.is_empty() {
            login.email.clone()
        } else {
            login.username.clone()
        };
        let host = login.urls.first().map_or("", |u| host(u)).to_owned();
        let mut fields = vec![
            Field::new(Weight::Title, &login.title),
            Field::new(Weight::Account, &login.username),
            Field::new(Weight::Account, &login.email),
        ];
        fields.extend(login.urls.iter().map(|u| Field::new(Weight::Url, u)));
        Self {
            login,
            vault,
            account,
            host,
            fields,
        }
    }

    /// Unique across vaults; the desktop hands it back when a result is picked.
    pub fn id(&self) -> String {
        format!("{}/{}", self.login.share_id, self.login.item_id)
    }

    /// `account · host · vault`, leaving out empty parts.
    pub fn subtext(&self) -> String {
        [
            self.account.as_str(),
            self.host.as_str(),
            self.vault.as_str(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

pub struct Snapshot {
    /// Sorted by title.
    pub entries: Vec<Arc<Entry>>,
    /// False until the first load attempt has finished.
    pub loaded: bool,
    /// The last load error. Entries from an earlier successful load are kept.
    pub error: Option<String>,
    pub refreshed_at: Instant,
}

impl Snapshot {
    pub fn find(&self, id: &str) -> Option<Arc<Entry>> {
        self.entries.iter().find(|e| e.id() == id).cloned()
    }

    /// The best `limit` entries for `query`, best first, with their scores.
    pub fn search(&self, query: &Query, limit: usize) -> Vec<(Arc<Entry>, u32)> {
        let mut hits: Vec<_> = self
            .entries
            .iter()
            .filter_map(|e| query.score(&e.fields).map(|score| (e.clone(), score)))
            .collect();
        // Stable, so equal scores keep title order.
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.1));
        hits.truncate(limit);
        hits
    }
}

pub struct Index {
    snapshot: watch::Sender<Arc<Snapshot>>,
    refreshing: Mutex<()>,
}

impl Index {
    pub fn new() -> Self {
        let empty = Snapshot {
            entries: Vec::new(),
            loaded: false,
            error: None,
            refreshed_at: Instant::now(),
        };
        Self {
            snapshot: watch::Sender::new(Arc::new(empty)),
            refreshing: Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.borrow().clone()
    }

    /// Waits up to `max` for the first load to finish, then returns the current snapshot.
    pub async fn wait_loaded(&self, max: Duration) -> Arc<Snapshot> {
        let mut rx = self.snapshot.subscribe();
        let _ = tokio::time::timeout(max, rx.wait_for(|s| s.loaded)).await;
        self.snapshot()
    }

    /// Starts a reload in the background if the data is at least `age` old.
    pub fn refresh_if_older_than(self: &Arc<Self>, age: Duration) {
        if self.snapshot().refreshed_at.elapsed() >= age {
            let index = self.clone();
            tokio::spawn(async move { index.refresh().await });
        }
    }

    /// Reloads all vaults. Returns at once if a reload is already running.
    pub async fn refresh(&self) {
        let Ok(_guard) = self.refreshing.try_lock() else {
            return;
        };
        let started = Instant::now();
        let next = match load().await {
            Ok(entries) => {
                eprintln!(
                    "loaded {} logins in {:.1?}",
                    entries.len(),
                    started.elapsed()
                );
                Snapshot {
                    entries,
                    loaded: true,
                    error: None,
                    refreshed_at: Instant::now(),
                }
            }
            Err(e) => {
                let error = format!("{e:#}");
                eprintln!("loading logins failed: {error}");
                Snapshot {
                    entries: self.snapshot().entries.clone(),
                    loaded: true,
                    error: Some(error),
                    refreshed_at: Instant::now(),
                }
            }
        };
        self.snapshot.send_replace(Arc::new(next));
    }
}

async fn load() -> Result<Vec<Arc<Entry>>> {
    let mut tasks = JoinSet::new();
    for vault in pass_cli::vaults().await? {
        tasks.spawn(async move {
            let logins = pass_cli::logins(&vault.share_id)
                .await
                .with_context(|| format!("vault {:?}", vault.name))?;
            Ok::<_, anyhow::Error>((vault.name, logins))
        });
    }
    let mut entries = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let (vault, logins) = joined.context("vault task panicked")??;
        entries.extend(
            logins
                .into_iter()
                .map(|login| Arc::new(Entry::new(login, vault.clone()))),
        );
    }
    entries.sort_by_cached_key(|e| e.login.title.to_lowercase());
    Ok(entries)
}

/// `https://www.example.com:8080/path` → `www.example.com`.
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
    &rest[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login_fields(title: &str) -> Login {
        Login {
            share_id: "s".into(),
            item_id: title.into(),
            title: title.into(),
            username: String::new(),
            email: "me@example.com".into(),
            urls: Vec::new(),
            has_totp: false,
        }
    }

    fn entry(title: &str, username: &str, urls: &[&str]) -> Arc<Entry> {
        let login = Login {
            username: username.into(),
            urls: urls.iter().map(|u| u.to_string()).collect(),
            ..login_fields(title)
        };
        Arc::new(Entry::new(login, "Personal".into()))
    }

    fn snapshot(entries: Vec<Arc<Entry>>) -> Snapshot {
        Snapshot {
            entries,
            loaded: true,
            error: None,
            refreshed_at: Instant::now(),
        }
    }

    #[test]
    fn host_strips_scheme_port_and_path() {
        assert_eq!(
            host("https://www.example.com:8080/a?b#c"),
            "www.example.com"
        );
        assert_eq!(host("example.com/login"), "example.com");
    }

    #[test]
    fn subtext_skips_empty_parts() {
        assert_eq!(
            entry("A", "alice", &["https://a.example/x"]).subtext(),
            "alice · a.example · Personal"
        );
        let mut no_email = Login {
            email: String::new(),
            ..login_fields("A")
        };
        no_email.username = String::new();
        assert_eq!(Entry::new(no_email, "Work".into()).subtext(), "Work");
    }

    #[test]
    fn find_by_id() {
        let s = snapshot(vec![entry("A", "", &[]), entry("B", "", &[])]);
        assert_eq!(s.find("s/B").unwrap().login.title, "B");
        assert!(s.find("s/C").is_none());
    }

    #[test]
    fn account_falls_back_to_email() {
        assert_eq!(entry("A", "", &[]).account, "me@example.com");
        assert_eq!(entry("A", "alice", &[]).account, "alice");
    }

    #[test]
    fn title_match_outranks_url_match() {
        let s = snapshot(vec![
            entry("Bank", "", &[]),
            entry("GitHub", "", &[]),
            entry("Work SSO", "", &["https://github.com/org"]),
        ]);
        let hits = s.search(&Query::parse("github"), 10);
        let titles: Vec<_> = hits.iter().map(|(e, _)| e.login.title.as_str()).collect();
        assert_eq!(titles, ["GitHub", "Work SSO"]);
    }

    #[test]
    fn limit_keeps_best() {
        let s = snapshot(vec![
            entry("git a", "", &[]),
            entry("git", "", &[]),
            entry("x", "", &[]),
        ]);
        let hits = s.search(&Query::parse("git"), 1);
        assert_eq!(hits[0].0.login.title, "git");
    }
}
