//! Runs the service in isolation: a private D-Bus bus, optionally a headless KWin for the
//! clipboard, a fake `pass-cli` serving `tests/fixtures/data`, and a fake notification server
//! that records what the service reports. Every process is killed when the [`Session`] drops.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use zbus::{Connection, OwnedVariant, interface};

pub const BUS_NAME: &str = "local.ProtonPassSearch";
const STARTUP: Duration = Duration::from_secs(15);

/// Id, text, icon name, category relevance, relevance, properties.
pub type RemoteMatch = (
    String,
    String,
    String,
    i32,
    f64,
    HashMap<String, OwnedVariant>,
);

#[derive(Clone, Debug)]
pub struct Notification {
    pub summary: String,
    pub body: String,
    pub transient: bool,
}

pub struct Options {
    /// Start a headless KWin and give the service its clipboard.
    pub wayland: bool,
    pub args: Vec<String>,
    /// Make every `pass-cli` call fail with this message, from the first one on.
    pub fail_with: Option<&'static str>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            wayland: false,
            args: vec!["--clear-after".into(), "1".into()],
            fail_with: None,
        }
    }
}

pub struct Session {
    dir: TempDir,
    pub data: PathBuf,
    address: String,
    wayland: Option<String>,
    pub conn: Connection,
    notifications: Arc<Mutex<Vec<Notification>>>,
    _notification_conn: Connection,
    // Killed in this order on drop.
    service: Child,
    kwin: Option<Child>,
    dbus: Child,
}

impl Session {
    pub async fn start(options: Options) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        copy_dir(&fixtures().join("data"), &data);
        if let Some(message) = options.fail_with {
            std::fs::write(data.join("fail"), message).unwrap();
        }

        let (dbus, address) = start_dbus(dir.path());
        let (kwin, wayland) = if options.wayland {
            let (kwin, socket) = start_kwin(&address, dir.path());
            (Some(kwin), Some(socket))
        } else {
            (None, None)
        };

        let notifications = Arc::new(Mutex::new(Vec::new()));
        let notification_conn = zbus::connection::Builder::address(address.as_str())
            .name("org.freedesktop.Notifications")
            .serve_at(
                "/org/freedesktop/Notifications",
                FakeNotifications {
                    log: notifications.clone(),
                },
            )
            .build()
            .await
            .unwrap();
        let conn = zbus::connection::Builder::address(address.as_str())
            .build()
            .await
            .unwrap();

        let mut command = Command::new(env!("CARGO_BIN_EXE_proton-pass-search"));
        command
            .args(&options.args)
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("FAKE_PASS_DIR", &data)
            .env("PATH", path_with_fake_pass_cli())
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(dir.path().join("service.log")).unwrap());
        match &wayland {
            Some(socket) => command.env("WAYLAND_DISPLAY", socket),
            None => command.env_remove("WAYLAND_DISPLAY"),
        };
        let service = command.spawn().unwrap();

        let session = Self {
            dir,
            data,
            address,
            wayland,
            conn,
            notifications,
            _notification_conn: notification_conn,
            service,
            kwin,
            dbus,
        };
        session.wait_for_service().await;
        session
    }

    async fn wait_for_service(&self) {
        let deadline = Instant::now() + STARTUP;
        while Instant::now() < deadline {
            let reply = self
                .conn
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "NameHasOwner",
                    &(BUS_NAME,),
                )
                .await
                .unwrap();
            if reply.body().deserialize::<bool>().unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "service did not claim {BUS_NAME}; log:\n{}",
            self.service_log()
        );
    }

    pub fn service_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("service.log")).unwrap_or_default()
    }

    /// The fake `pass-cli` calls so far, one per line.
    pub fn pass_cli_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.data.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    async fn call<B, R>(&self, path: &str, interface: &str, method: &str, body: &B) -> R
    where
        B: serde::Serialize + zbus::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::Type,
    {
        let reply = self
            .conn
            .call_method(Some(BUS_NAME), path, Some(interface), method, body)
            .await
            .unwrap_or_else(|e| panic!("{method} failed: {e}; log:\n{}", self.service_log()));
        reply.body().deserialize().unwrap()
    }

    pub async fn krunner_config(&self) -> HashMap<String, OwnedVariant> {
        self.call("/krunner", "org.kde.krunner1", "Config", &())
            .await
    }

    pub async fn krunner_actions(&self) -> Vec<(String, String, String)> {
        self.call("/krunner", "org.kde.krunner1", "Actions", &())
            .await
    }

    pub async fn krunner_match(&self, query: &str) -> Vec<RemoteMatch> {
        self.call("/krunner", "org.kde.krunner1", "Match", &(query,))
            .await
    }

    pub async fn krunner_run(&self, match_id: &str, action_id: &str) {
        self.call::<_, ()>(
            "/krunner",
            "org.kde.krunner1",
            "Run",
            &(match_id, action_id),
        )
        .await
    }

    pub async fn krunner_teardown(&self) {
        self.call::<_, ()>("/krunner", "org.kde.krunner1", "Teardown", &())
            .await
    }

    pub async fn gnome_initial(&self, terms: &str) -> Vec<String> {
        let terms: Vec<&str> = terms.split_whitespace().collect();
        self.call(
            "/gnome",
            "org.gnome.Shell.SearchProvider2",
            "GetInitialResultSet",
            &(terms,),
        )
        .await
    }

    pub async fn gnome_subsearch(&self, previous: &[String], terms: &str) -> Vec<String> {
        let terms: Vec<&str> = terms.split_whitespace().collect();
        self.call(
            "/gnome",
            "org.gnome.Shell.SearchProvider2",
            "GetSubsearchResultSet",
            &(previous, terms),
        )
        .await
    }

    pub async fn gnome_metas(&self, ids: &[String]) -> Vec<HashMap<String, OwnedVariant>> {
        self.call(
            "/gnome",
            "org.gnome.Shell.SearchProvider2",
            "GetResultMetas",
            &(ids,),
        )
        .await
    }

    pub async fn gnome_activate(&self, id: &str) {
        let terms: Vec<&str> = Vec::new();
        self.call::<_, ()>(
            "/gnome",
            "org.gnome.Shell.SearchProvider2",
            "ActivateResult",
            &(id, terms, 0u32),
        )
        .await
    }

    /// Waits for the next notification the service sends.
    pub async fn next_notification(&self) -> Notification {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(n) = {
                let mut log = self.notifications.lock().unwrap();
                (!log.is_empty()).then(|| log.remove(0))
            } {
                return n;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no notification; log:\n{}", self.service_log());
    }

    pub fn notification_count(&self) -> usize {
        self.notifications.lock().unwrap().len()
    }

    fn wl(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env(
                "WAYLAND_DISPLAY",
                self.wayland.as_ref().expect("session has no Wayland"),
            )
            .env_remove("DISPLAY");
        command
    }

    /// The clipboard text, or `None` when the clipboard is empty.
    pub fn paste(&self) -> Option<String> {
        let out = self
            .wl("wl-paste")
            .args(["--no-newline", "--type", "text/plain"])
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8(out.stdout).unwrap())
    }

    pub fn paste_types(&self) -> Vec<String> {
        let out = self.wl("wl-paste").arg("--list-types").output().unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Copies like another application would. The forked `wl-copy` exits with KWin.
    pub fn copy_as_other_app(&self, text: &str) {
        let status = self.wl("wl-copy").arg(text).status().unwrap();
        assert!(status.success());
    }

    /// Polls `f` until it returns `Some`, for up to `max`.
    pub async fn eventually<T>(&self, max: Duration, mut f: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + max;
        loop {
            if let Some(v) = f() {
                return v;
            }
            if Instant::now() > deadline {
                panic!(
                    "condition not met within {max:?}; log:\n{}",
                    self.service_log()
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for child in [
            Some(&mut self.service),
            self.kwin.as_mut(),
            Some(&mut self.dbus),
        ]
        .into_iter()
        .flatten()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct FakeNotifications {
    log: Arc<Mutex<Vec<Notification>>>,
}

#[interface(name = "org.freedesktop.Notifications")]
impl FakeNotifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        _expire_timeout: i32,
    ) -> u32 {
        let transient = hints
            .get("transient")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);
        let mut log = self.log.lock().unwrap();
        log.push(Notification {
            summary,
            body,
            transient,
        });
        log.len() as u32
    }
}

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn path_with_fake_pass_cli() -> String {
    format!(
        "{}:{}",
        fixtures().join("bin").display(),
        std::env::var("PATH").unwrap()
    )
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

/// A bus with no service files, so nothing gets activated behind the test's back.
fn start_dbus(dir: &Path) -> (Child, String) {
    let config = dir.join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>"#,
            dir.display()
        ),
    )
    .unwrap();
    let mut dbus = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("starting `dbus-daemon` failed: {e}"));
    let mut address = String::new();
    BufReader::new(dbus.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    (dbus, address.trim().to_owned())
}

fn start_kwin(address: &str, dir: &Path) -> (Child, String) {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let socket = format!(
        "pps-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let kwin = Command::new("kwin_wayland")
        .args(["--virtual", "--no-lockscreen", "--socket", &socket])
        .env("DBUS_SESSION_BUS_ADDRESS", address)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(dir.join("kwin.log")).unwrap())
        .spawn()
        .unwrap_or_else(|e| {
            panic!("clipboard tests run on a headless KWin; starting `kwin_wayland` failed: {e}")
        });
    let runtime_dir = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap());
    let deadline = Instant::now() + STARTUP;
    while !runtime_dir.join(&socket).exists() {
        assert!(Instant::now() < deadline, "KWin did not create {socket}");
        std::thread::sleep(Duration::from_millis(50));
    }
    (kwin, socket)
}
