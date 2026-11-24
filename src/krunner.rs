//! The `org.kde.krunner1` D-Bus interface, as described in KRunner's
//! `kf6_org.kde.krunner1.xml`.
//!
//! KRunner sends a query here only when it starts with [`PREFIX`], or always in single-runner
//! mode, where the prefix is optional. Enter copies the password; the action buttons copy the
//! username or the current one-time code.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use zbus::{Connection, Variant, interface};

use crate::actions::{self, CopySettings, Field};
use crate::index::{Entry, Index};
use crate::search::Query;

/// Plain text, so it goes into the match regex unescaped.
pub const PREFIX: &str = "pp ";
pub const MAX_RESULTS: usize = 15;
/// Below KRunner's 25 s D-Bus call timeout.
pub const FIRST_LOAD_WAIT: Duration = Duration::from_secs(20);
const ERROR_MATCH_ID: &str = "error";
const ACTION_USERNAME: &str = "username";
const ACTION_TOTP: &str = "totp";
/// `KRunner::QueryMatch::CategoryRelevance::Highest`.
const CATEGORY_RELEVANCE_HIGHEST: i32 = 100;
const CATEGORY: &str = "Proton Pass";

/// Id, text, icon name, category relevance, relevance, properties.
type RemoteMatch = (
    String,
    String,
    String,
    i32,
    f64,
    HashMap<String, Variant<'static>>,
);

pub struct Runner {
    index: Arc<Index>,
    copy: CopySettings,
    refresh_after: Duration,
}

impl Runner {
    pub fn new(index: Arc<Index>, copy: CopySettings, refresh_after: Duration) -> Self {
        Self {
            index,
            copy,
            refresh_after,
        }
    }
}

#[interface(name = "org.kde.krunner1")]
impl Runner {
    fn config(&self) -> HashMap<String, Variant<'static>> {
        HashMap::from([("MatchRegex".to_owned(), Variant::from(format!("^{PREFIX}")))])
    }

    fn actions(&self) -> Vec<(String, String, String)> {
        let action = |id: &str, text: &str, icon: &str| (id.into(), text.into(), icon.into());
        vec![
            action(ACTION_USERNAME, "Copy username", "user-identity"),
            action(ACTION_TOTP, "Copy one-time code", "chronometer"),
        ]
    }

    #[zbus(name = "Match")]
    async fn match_query(&self, query: String) -> Vec<RemoteMatch> {
        let query = Query::parse(query.strip_prefix(PREFIX).unwrap_or(&query));
        if query.is_empty() {
            return Vec::new();
        }
        let snapshot = self.index.wait_loaded(FIRST_LOAD_WAIT).await;
        if snapshot.entries.is_empty() {
            return match &snapshot.error {
                Some(error) => vec![error_match(error)],
                None => Vec::new(),
            };
        }
        let hits = snapshot.search(&query, MAX_RESULTS);
        let scores: Vec<u32> = hits.iter().map(|(_, score)| *score).collect();
        hits.iter()
            .zip(relevances(&scores))
            .map(|((entry, _), relevance)| remote_match(entry, relevance))
            .collect()
    }

    /// Returns at once; the copy runs in the background so KRunner can close.
    fn run(&self, match_id: String, action_id: String, #[zbus(connection)] conn: &Connection) {
        if match_id == ERROR_MATCH_ID {
            return;
        }
        let field = match action_id.as_str() {
            "" => Field::Password,
            ACTION_USERNAME => Field::Username,
            ACTION_TOTP => Field::Totp,
            other => {
                eprintln!("run: unknown action {other:?}");
                return;
            }
        };
        match self.index.snapshot().find(&match_id) {
            Some(entry) => actions::copy_and_notify(conn, self.copy, &entry, field),
            None => eprintln!("run: no entry with id {match_id}"),
        }
    }

    /// Called when a KRunner session ends; reloads the vaults if the data is old.
    fn teardown(&self) {
        self.index.refresh_if_older_than(self.refresh_after);
    }

    /// Needed only by runners that open windows.
    fn set_activation_token(&self, _token: String) {}
}

/// Maps scores, best first, to KRunner relevances in (0, 1].
///
/// KRunner reorders matches of equal relevance, so each one gets slightly less than the one
/// before it; the gap is too small to change the order of different scores.
fn relevances(scores: &[u32]) -> Vec<f64> {
    let best = scores.first().copied().unwrap_or(1).max(1) as f64;
    scores
        .iter()
        .enumerate()
        .map(|(rank, &score)| score as f64 / best * (1.0 - rank as f64 * 1e-6))
        .collect()
}

fn remote_match(entry: &Entry, relevance: f64) -> RemoteMatch {
    let actions: Vec<String> = [
        (Field::Username, ACTION_USERNAME),
        (Field::Totp, ACTION_TOTP),
    ]
    .into_iter()
    .filter(|(field, _)| field.available(entry))
    .map(|(_, id)| id.to_owned())
    .collect();
    let properties = HashMap::from([
        ("subtext".to_owned(), Variant::from(entry.subtext())),
        ("actions".to_owned(), Variant::from(actions)),
        ("category".to_owned(), Variant::from(CATEGORY.to_owned())),
    ]);
    (
        entry.id(),
        entry.login.title.clone(),
        "dialog-password".to_owned(),
        CATEGORY_RELEVANCE_HIGHEST,
        relevance,
        properties,
    )
}

fn error_match(error: &str) -> RemoteMatch {
    let properties = HashMap::from([
        ("subtext".to_owned(), Variant::from(error.to_owned())),
        ("actions".to_owned(), Variant::from(Vec::<String>::new())),
        ("category".to_owned(), Variant::from(CATEGORY.to_owned())),
    ]);
    (
        ERROR_MATCH_ID.to_owned(),
        "Proton Pass: could not load logins".to_owned(),
        "dialog-error".to_owned(),
        CATEGORY_RELEVANCE_HIGHEST,
        1.0,
        properties,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevances_strictly_decrease_and_keep_proportions() {
        let r = relevances(&[4000, 4000, 4000, 2000, 1000]);
        assert!(r.windows(2).all(|w| w[0] > w[1]), "{r:?}");
        assert_eq!(r[0], 1.0);
        assert!((r[3] - 0.5).abs() < 1e-5);
        assert!((r[4] - 0.25).abs() < 1e-5);
    }

    #[test]
    fn relevances_of_nothing() {
        assert!(relevances(&[]).is_empty());
    }
}
