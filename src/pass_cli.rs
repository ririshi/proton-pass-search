//! Calls Proton's `pass-cli` and parses its JSON output.
//!
//! Commands run with an argument list, never through a shell. Output buffers are wiped when
//! dropped, and listings keep no passwords or TOTP secrets: the parser skips those fields.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer};
use tokio::process::Command;
use zeroize::Zeroizing;

const PASS_CLI: &str = "pass-cli";

pub struct Vault {
    pub name: String,
    pub share_id: String,
}

/// A login item, without its password or TOTP secret.
pub struct Login {
    pub share_id: String,
    pub item_id: String,
    pub title: String,
    pub username: String,
    pub email: String,
    pub urls: Vec<String>,
    pub has_totp: bool,
}

pub async fn vaults() -> Result<Vec<Vault>> {
    parse_vaults(&run(&["vault", "list", "--output", "json"]).await?)
}

fn parse_vaults(json: &[u8]) -> Result<Vec<Vault>> {
    #[derive(Deserialize)]
    struct VaultList {
        vaults: Vec<VaultJson>,
    }
    #[derive(Deserialize)]
    struct VaultJson {
        name: String,
        share_id: String,
    }

    let list: VaultList =
        serde_json::from_slice(json).context("parsing `pass-cli vault list` output")?;
    Ok(list
        .vaults
        .into_iter()
        .map(|v| Vault {
            name: v.name,
            share_id: v.share_id,
        })
        .collect())
}

/// Lists the active login items in one vault.
///
/// Usernames, emails and URLs appear only with `--show-secrets`, which also prints passwords.
pub async fn logins(share_id: &str) -> Result<Vec<Login>> {
    let out = run(&[
        "item",
        "list",
        "--share-id",
        share_id,
        "--filter-state",
        "active",
        "--filter-type",
        "login",
        "--output",
        "json",
        "--show-secrets",
    ])
    .await?;
    parse_logins(&out)
}

/// Keeps active login items; other types and trashed items are dropped.
fn parse_logins(json: &[u8]) -> Result<Vec<Login>> {
    #[derive(Deserialize)]
    struct ItemList {
        items: Vec<ItemJson>,
    }
    #[derive(Deserialize)]
    struct ItemJson {
        id: String,
        share_id: String,
        state: String,
        content: ContentJson,
    }
    #[derive(Deserialize)]
    struct ContentJson {
        title: String,
        content: KindJson,
    }
    /// The item type is the single key of this object, such as `Login` or `Note`.
    #[derive(Deserialize)]
    struct KindJson {
        #[serde(rename = "Login")]
        login: Option<LoginJson>,
    }
    #[derive(Deserialize)]
    struct LoginJson {
        username: String,
        email: String,
        urls: Vec<String>,
        #[serde(rename = "totp_uri", deserialize_with = "is_non_empty_secret")]
        has_totp: bool,
    }

    let list: ItemList =
        serde_json::from_slice(json).context("parsing `pass-cli item list` output")?;
    Ok(list
        .items
        .into_iter()
        .filter(|item| item.state == "Active")
        .filter_map(|item| {
            let login = item.content.content.login?;
            Some(Login {
                share_id: item.share_id,
                item_id: item.id,
                title: item.content.title,
                username: login.username,
                email: login.email,
                urls: login.urls,
                has_totp: login.has_totp,
            })
        })
        .collect())
}

pub async fn password(share_id: &str, item_id: &str) -> Result<Zeroizing<String>> {
    let uri = format!("pass://{share_id}/{item_id}/password");
    parse_field(&run(&["item", "view", &uri]).await?)
}

/// `pass-cli item view` ends its output with a newline that is not part of the field.
fn parse_field(out: &[u8]) -> Result<Zeroizing<String>> {
    let text = std::str::from_utf8(out).context("`pass-cli item view` printed invalid UTF-8")?;
    Ok(Zeroizing::new(
        text.strip_suffix('\n').unwrap_or(text).to_owned(),
    ))
}

/// The current one-time code of an item.
pub async fn totp(share_id: &str, item_id: &str) -> Result<Zeroizing<String>> {
    let uri = format!("pass://{share_id}/{item_id}");
    parse_totp(&run(&["item", "totp", &uri, "--output", "json"]).await?)
}

fn parse_totp(json: &[u8]) -> Result<Zeroizing<String>> {
    #[derive(Deserialize)]
    struct TotpJson {
        #[serde(deserialize_with = "zeroizing_string")]
        totp: Zeroizing<String>,
    }

    let parsed: TotpJson =
        serde_json::from_slice(json).context("parsing `pass-cli item totp` output")?;
    Ok(parsed.totp)
}

async fn run(args: &[&str]) -> Result<Zeroizing<Vec<u8>>> {
    let out = Command::new(PASS_CLI)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("starting `{PASS_CLI}`"))?;
    let stdout = Zeroizing::new(out.stdout);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "`{PASS_CLI} {} {}` failed: {}",
            args[0],
            args[1],
            stderr.trim()
        );
    }
    Ok(stdout)
}

fn zeroizing_string<'de, D: Deserializer<'de>>(d: D) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(d).map(Zeroizing::new)
}

fn is_non_empty_secret<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    zeroizing_string(d).map(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERSONAL: &[u8] = include_bytes!("../tests/fixtures/data/items-share-personal.json");

    #[test]
    fn vaults_parse() {
        let vaults = parse_vaults(include_bytes!("../tests/fixtures/data/vaults.json")).unwrap();
        let names: Vec<_> = vaults
            .iter()
            .map(|v| (v.name.as_str(), v.share_id.as_str()))
            .collect();
        assert_eq!(
            names,
            [("Personal", "share-personal"), ("Work", "share-work")]
        );
    }

    #[test]
    fn logins_keep_active_logins_only() {
        let logins = parse_logins(PERSONAL).unwrap();
        let titles: Vec<_> = logins.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(titles, ["GitHub", "Bänk"]);
    }

    #[test]
    fn logins_carry_fields_but_no_secrets() {
        let logins = parse_logins(PERSONAL).unwrap();
        let github = &logins[0];
        assert_eq!(
            (github.share_id.as_str(), github.item_id.as_str()),
            ("share-personal", "item-github")
        );
        assert_eq!(github.username, "alice");
        assert_eq!(github.email, "alice@example.com");
        assert_eq!(github.urls, ["https://github.com/login"]);
        assert!(github.has_totp);
        assert!(!logins[1].has_totp);
    }

    #[test]
    fn logins_reject_unexpected_shape() {
        assert!(parse_logins(br#"{"items": [{"id": 1}]}"#).is_err());
    }

    #[test]
    fn field_drops_one_trailing_newline() {
        assert_eq!(*parse_field(b"secret\n").unwrap(), "secret");
        assert_eq!(*parse_field(b"secret").unwrap(), "secret");
        assert_eq!(
            *parse_field(b"ends with newline\n\n").unwrap(),
            "ends with newline\n"
        );
    }

    #[test]
    fn totp_parses() {
        let json = include_bytes!("../tests/fixtures/data/totp-item-github.json");
        assert_eq!(*parse_totp(json).unwrap(), "123456");
    }
}
