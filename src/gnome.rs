//! The `org.gnome.Shell.SearchProvider2` D-Bus interface, for the GNOME Shell overview search.
//!
//! GNOME sends every search to every provider and splits it into words, so results appear only
//! when the first word is [`PREFIX`]. GNOME results have no action buttons; a second word picks
//! the field to copy instead:
//!
//! - `pp github` copies the password.
//! - `pp u github` copies the username.
//! - `pp t github` copies the one-time code.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use zbus::{Connection, Variant, interface};

use crate::actions::{self, CopySettings, Field};
use crate::index::Index;
use crate::krunner::{FIRST_LOAD_WAIT, MAX_RESULTS};
use crate::search::Query;

pub const PREFIX: &str = "pp";
const MODE_USERNAME: &str = "u";
const MODE_TOTP: &str = "t";
/// A `GThemedIcon` with a fallback: Breeze has the first name, Adwaita only the second.
const ICON: &str = ". GThemedIcon dialog-password dialog-password-symbolic";

pub struct SearchProvider {
    index: Arc<Index>,
    copy: CopySettings,
    refresh_after: Duration,
}

impl SearchProvider {
    pub fn new(index: Arc<Index>, copy: CopySettings, refresh_after: Duration) -> Self {
        Self {
            index,
            copy,
            refresh_after,
        }
    }

    async fn results(&self, terms: &[String]) -> Vec<String> {
        let Some((field, query)) = parse_terms(terms) else {
            return Vec::new();
        };
        let query = Query::parse(&query);
        if query.is_empty() {
            return Vec::new();
        }
        let snapshot = self.index.wait_loaded(FIRST_LOAD_WAIT).await;
        self.index.refresh_if_older_than(self.refresh_after);
        snapshot
            .search(&query, MAX_RESULTS)
            .into_iter()
            .filter(|(entry, _)| field.available(entry))
            .map(|(entry, _)| result_id(field, &entry.id()))
            .collect()
    }
}

#[interface(name = "org.gnome.Shell.SearchProvider2")]
impl SearchProvider {
    async fn get_initial_result_set(&self, terms: Vec<String>) -> Vec<String> {
        self.results(&terms).await
    }

    /// The index is small, so searching again is simpler than filtering `previous_results`.
    async fn get_subsearch_result_set(
        &self,
        _previous_results: Vec<String>,
        terms: Vec<String>,
    ) -> Vec<String> {
        self.results(&terms).await
    }

    fn get_result_metas(&self, identifiers: Vec<String>) -> Vec<HashMap<String, Variant<'static>>> {
        let snapshot = self.index.snapshot();
        identifiers
            .iter()
            .filter_map(|id| {
                let (field, entry_id) = parse_result_id(id)?;
                let entry = snapshot.find(entry_id)?;
                let description = format!("Copy {} · {}", field.describe(), entry.subtext());
                Some(HashMap::from([
                    ("id".to_owned(), Variant::from(id.clone())),
                    ("name".to_owned(), Variant::from(entry.login.title.clone())),
                    ("description".to_owned(), Variant::from(description)),
                    ("gicon".to_owned(), Variant::from(ICON.to_owned())),
                ]))
            })
            .collect()
    }

    fn activate_result(
        &self,
        identifier: String,
        _terms: Vec<String>,
        _timestamp: u32,
        #[zbus(connection)] conn: &Connection,
    ) {
        let Some((field, entry_id)) = parse_result_id(&identifier) else {
            eprintln!("activate: malformed id {identifier}");
            return;
        };
        match self.index.snapshot().find(entry_id) {
            Some(entry) => actions::copy_and_notify(conn, self.copy, &entry, field),
            None => eprintln!("activate: no entry with id {entry_id}"),
        }
    }

    /// Clicking the provider's icon. There is no window to open.
    fn launch_search(&self, _terms: Vec<String>, _timestamp: u32) {}
}

/// `["pp", "u", "git", "hub"]` → `(Username, "git hub")`. `None` without the prefix.
fn parse_terms(terms: &[String]) -> Option<(Field, String)> {
    let (first, rest) = terms.split_first()?;
    if !first.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let (field, rest) = match rest.split_first() {
        Some((mode, more)) if !more.is_empty() && mode.eq_ignore_ascii_case(MODE_USERNAME) => {
            (Field::Username, more)
        }
        Some((mode, more)) if !more.is_empty() && mode.eq_ignore_ascii_case(MODE_TOTP) => {
            (Field::Totp, more)
        }
        _ => (Field::Password, rest),
    };
    Some((field, rest.join(" ")))
}

fn result_id(field: Field, entry_id: &str) -> String {
    let tag = match field {
        Field::Password => "p",
        Field::Username => "u",
        Field::Totp => "t",
    };
    format!("{tag}:{entry_id}")
}

fn parse_result_id(id: &str) -> Option<(Field, &str)> {
    let (tag, entry_id) = id.split_once(':')?;
    let field = match tag {
        "p" => Field::Password,
        "u" => Field::Username,
        "t" => Field::Totp,
        _ => return None,
    };
    Some((field, entry_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn needs_prefix() {
        assert_eq!(parse_terms(&terms("github")), None);
        assert_eq!(
            parse_terms(&terms("PP github")),
            Some((Field::Password, "github".into()))
        );
    }

    #[test]
    fn mode_words() {
        assert_eq!(
            parse_terms(&terms("pp u git hub")),
            Some((Field::Username, "git hub".into()))
        );
        assert_eq!(
            parse_terms(&terms("pp t github")),
            Some((Field::Totp, "github".into()))
        );
    }

    #[test]
    fn lone_mode_letter_is_a_search_word() {
        assert_eq!(
            parse_terms(&terms("pp t")),
            Some((Field::Password, "t".into()))
        );
    }

    #[test]
    fn result_ids_round_trip() {
        for field in [Field::Password, Field::Username, Field::Totp] {
            let id = result_id(field, "share/item");
            assert_eq!(parse_result_id(&id), Some((field, "share/item")));
        }
        assert_eq!(parse_result_id("x:share/item"), None);
        assert_eq!(parse_result_id("nocolon"), None);
    }
}
