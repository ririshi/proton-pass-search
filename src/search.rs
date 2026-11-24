//! Ranks login items against a query the way Proton Pass's own search does.
//!
//! Every word of the query must occur in some field of an item. Each word scores the best
//! `field weight × match quality` over the item's fields, and the item scores the sum over the
//! words. Weights make any title match outrank any other match; quality prefers an exact field,
//! then a prefix, then the start of a word inside the field, then any substring. Matching ignores
//! case and accents.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Title = 1000,
    Account = 100,
    Url = 50,
}

/// A field prepared for matching.
pub struct Field {
    weight: Weight,
    text: String,
}

impl Field {
    pub fn new(weight: Weight, text: &str) -> Self {
        Self {
            weight,
            text: normalize(text),
        }
    }
}

/// The query split into normalized, distinct words.
pub struct Query {
    words: Vec<String>,
}

impl Query {
    pub fn parse(raw: &str) -> Self {
        let mut words: Vec<String> = Vec::new();
        for word in normalize(raw).split_whitespace() {
            if !words.iter().any(|w| w == word) {
                words.push(word.to_owned());
            }
        }
        Self { words }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The item's score, or `None` if some word occurs in none of its fields.
    pub fn score(&self, fields: &[Field]) -> Option<u32> {
        self.words.iter().try_fold(0, |total, word| {
            let best = fields
                .iter()
                .map(|f| f.weight as u32 * quality(&f.text, word) as u32)
                .max()
                .unwrap_or(0);
            (best > 0).then_some(total + best)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Quality {
    None = 0,
    Substring = 1,
    WordPrefix = 2,
    Prefix = 4,
    Exact = 8,
}

fn quality(haystack: &str, needle: &str) -> Quality {
    if haystack == needle {
        return Quality::Exact;
    }
    if haystack.starts_with(needle) {
        return Quality::Prefix;
    }
    let mut found = Quality::None;
    for (index, _) in haystack.match_indices(needle) {
        let before = haystack[..index].chars().next_back();
        if before.is_some_and(is_word_boundary) {
            return Quality::WordPrefix;
        }
        found = Quality::Substring;
    }
    found
}

fn is_word_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | '@' | ':' | ',')
}

/// Lowercase, trimmed, with accents removed: `" Zoë "` → `"zoe"`.
fn normalize(text: &str) -> String {
    text.trim()
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_grades() {
        assert_eq!(quality("github", "github"), Quality::Exact);
        assert_eq!(quality("github work", "git"), Quality::Prefix);
        assert_eq!(quality("work github", "git"), Quality::WordPrefix);
        assert_eq!(quality("me@github.com", "github"), Quality::WordPrefix);
        assert_eq!(quality("legit", "git"), Quality::Substring);
        assert_eq!(quality("gitlab", "hub"), Quality::None);
    }

    #[test]
    fn word_prefix_found_after_earlier_substring() {
        assert_eq!(quality("legit git", "git"), Quality::WordPrefix);
    }

    #[test]
    fn normalize_ignores_case_and_accents() {
        assert_eq!(normalize("  Zoë Café "), "zoe cafe");
    }

    #[test]
    fn every_word_must_match() {
        let fields = [
            Field::new(Weight::Title, "GitHub"),
            Field::new(Weight::Account, "alice"),
        ];
        assert!(Query::parse("git alice").score(&fields).is_some());
        assert!(Query::parse("git bob").score(&fields).is_none());
    }

    #[test]
    fn substring_title_beats_exact_account() {
        let title = [Field::new(Weight::Title, "Protonmail")];
        let account = [Field::new(Weight::Account, "proton")];
        let query = Query::parse("proton");
        assert!(query.score(&title) > query.score(&account));
    }

    #[test]
    fn repeated_words_count_once() {
        let fields = [Field::new(Weight::Title, "GitHub")];
        assert_eq!(
            Query::parse("git git").score(&fields),
            Query::parse("git").score(&fields)
        );
    }
}
