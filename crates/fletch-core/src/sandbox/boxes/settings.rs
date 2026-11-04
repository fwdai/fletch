//! The box engine's settings (the `box_*` knobs), mirrored in-process: the
//! spawn path and the registry have no DB handle. Seeded at boot and kept
//! in sync by `commands::set_box_setting_impl` / `box_egress_allow_impl`.

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::RwLock;
use rusqlite::Connection;

use crate::database;
use crate::error::{Error, Result};

/// Guest RAM cap per box, MiB.
pub const MEMORY_SETTING: &str = "box_memory_mib";
/// vCPUs per box.
pub const CPUS_SETTING: &str = "box_cpus";
/// Data disk of each new box, GiB. Sparse, so it costs only what is written.
pub const DISK_SETTING: &str = "box_disk_gib";
/// Alternate base URL for the image download; empty is the GitHub release.
pub const IMAGE_URL_SETTING: &str = "box_image_url";
/// Minutes a box with no attached launch stays up; `0` is never.
pub const IDLE_STOP_SETTING: &str = "box_idle_stop_minutes";
/// What a box does with a host off its allow-list ([`EgressMode`]): `open`,
/// `ask` or `allowlist`. Absent is open.
pub const EGRESS_MODE_SETTING: &str = "box_egress_mode";
/// Project setting: a JSON array of host patterns added to the default egress
/// allow-list.
pub const EGRESS_ALLOW_KEY: &str = "box.egress_allow";
/// Project setting: the project's own [`EgressMode`] over the global one, in
/// the same spelling; absent is the global. A host-owned row, never read from
/// `.fletch/settings.json`: an agent can edit a file in its repository, and a
/// mode it could set to open would widen its own sandbox.
pub const EGRESS_MODE_KEY: &str = "sandbox.egress_mode";

/// Every global key, for the host-owned allowlist.
pub const KEYS: &[&str] = &[
    MEMORY_SETTING,
    CPUS_SETTING,
    DISK_SETTING,
    IMAGE_URL_SETTING,
    IDLE_STOP_SETTING,
    EGRESS_MODE_SETTING,
];

/// What a box does with a host its allow-list does not name. The posture a
/// user picks on Settings › Sandbox › Network, and a project may override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EgressMode {
    /// Any host on an allowed port passes; every connection is recorded.
    #[default]
    Open,
    /// The connection is held while the user is asked, once per host; nobody
    /// answering ends in a refusal.
    Ask,
    /// Refused at once, no prompt.
    AllowList,
}

impl EgressMode {
    /// `open`, `ask` or `allowlist`; blank is open; anything else is refused.
    pub fn parse(raw: Option<&str>) -> Result<EgressMode> {
        match raw.map(str::trim).filter(|s| !s.is_empty()) {
            None | Some("open") => Ok(EgressMode::Open),
            Some("ask") => Ok(EgressMode::Ask),
            Some("allowlist") => Ok(EgressMode::AllowList),
            Some(other) => Err(Error::Other(format!(
                "{EGRESS_MODE_SETTING} must be open, ask or allowlist; got {other:?}"
            ))),
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            EgressMode::Open => "open",
            EgressMode::Ask => "ask",
            EgressMode::AllowList => "allowlist",
        }
    }

    /// The proxy's own spelling of the same posture.
    pub fn proxy_mode(self) -> fletch_box::EgressMode {
        match self {
            EgressMode::Open => fletch_box::EgressMode::Open,
            EgressMode::Ask => fletch_box::EgressMode::Hold,
            EgressMode::AllowList => fletch_box::EgressMode::Refuse,
        }
    }
}

pub const DEFAULT_MEMORY_MIB: u32 = 2048;
pub const MIN_MEMORY_MIB: u32 = 512;
pub const DEFAULT_CPUS: u8 = 2;
pub const DEFAULT_DISK_GIB: u32 = (fletch_box::disks::DEFAULT_DATA_DISK_BYTES >> 30) as u32;
pub const MIN_DISK_GIB: u32 = 4;
pub const MAX_DISK_GIB: u32 = 256;
pub const DEFAULT_IDLE_STOP_MINUTES: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSettings {
    pub memory_mib: u32,
    pub cpus: u8,
    /// A box's disk is sized when it is created, so this reaches new boxes only.
    pub disk_gib: u32,
    /// `None` is the GitHub release for this build's pin.
    pub image_url: Option<String>,
    /// `None` is never.
    pub idle_stop: Option<Duration>,
    /// The global posture; a project may override it ([`egress_mode`]).
    pub egress_mode: EgressMode,
}

impl Default for BoxSettings {
    fn default() -> Self {
        Self {
            memory_mib: DEFAULT_MEMORY_MIB,
            cpus: DEFAULT_CPUS,
            disk_gib: DEFAULT_DISK_GIB,
            image_url: None,
            idle_stop: Some(Duration::from_secs(DEFAULT_IDLE_STOP_MINUTES * 60)),
            egress_mode: EgressMode::Open,
        }
    }
}

static SETTINGS: RwLock<BoxSettings> = RwLock::new(BoxSettings {
    memory_mib: DEFAULT_MEMORY_MIB,
    cpus: DEFAULT_CPUS,
    disk_gib: DEFAULT_DISK_GIB,
    image_url: None,
    idle_stop: Some(Duration::from_secs(DEFAULT_IDLE_STOP_MINUTES * 60)),
    egress_mode: EgressMode::Open,
});

/// `project_id → patterns`, the per-project egress additions.
static EGRESS: RwLock<Option<HashMap<String, Vec<String>>>> = RwLock::new(None);

/// `project_id → mode`, the projects that override the global posture.
static EGRESS_MODES: RwLock<Option<HashMap<String, EgressMode>>> = RwLock::new(None);

/// The user's global egress changes over the template
/// (`allowlist::GLOBAL_SETTING`).
static GLOBAL_EGRESS: RwLock<Option<super::allowlist::Delta>> = RwLock::new(None);

/// Held by every test that sets the box settings: they are process-wide, so
/// such tests run one at a time. Async-aware so an async test
/// may hold it across its awaits; a sync test takes `blocking_lock`.
#[cfg(test)]
pub(crate) static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn current() -> BoxSettings {
    SETTINGS.read().clone()
}

pub fn set(settings: BoxSettings) {
    *SETTINGS.write() = settings;
}

/// The settings as the table holds them. Each raw value goes through the same
/// parser the setter validates with, so a hand-edited row reads as its default
/// rather than as something the setter would have refused.
pub fn load(conn: &Connection) -> BoxSettings {
    let raw = |key: &str| database::get_setting(conn, key);
    BoxSettings {
        memory_mib: parse_memory(raw(MEMORY_SETTING).as_deref()).unwrap_or(DEFAULT_MEMORY_MIB),
        cpus: parse_cpus(raw(CPUS_SETTING).as_deref()).unwrap_or(DEFAULT_CPUS),
        disk_gib: parse_disk(raw(DISK_SETTING).as_deref()).unwrap_or(DEFAULT_DISK_GIB),
        image_url: parse_image_url(raw(IMAGE_URL_SETTING).as_deref()).unwrap_or(None),
        idle_stop: parse_idle_stop(raw(IDLE_STOP_SETTING).as_deref())
            .unwrap_or(Some(Duration::from_secs(DEFAULT_IDLE_STOP_MINUTES * 60))),
        egress_mode: EgressMode::parse(raw(EGRESS_MODE_SETTING).as_deref()).unwrap_or_default(),
    }
}

/// Seed every mirror from the database (boot).
pub fn seed(conn: &Connection) {
    set(load(conn));
    set_global_egress(super::allowlist::Delta::parse(
        database::get_setting(conn, super::allowlist::GLOBAL_SETTING).as_deref(),
    ));
    let mut allow = HashMap::new();
    for (project_id, value) in project_rows(conn, EGRESS_ALLOW_KEY) {
        allow.insert(project_id, parse_egress_allow(&value));
    }
    *EGRESS.write() = Some(allow);
    let mut modes = HashMap::new();
    for (project_id, value) in project_rows(conn, EGRESS_MODE_KEY) {
        // A hand-edited row reads as no override rather than as a posture the
        // setter would have refused.
        if let Ok(mode) = EgressMode::parse(Some(&value)) {
            modes.insert(project_id, mode);
        }
    }
    *EGRESS_MODES.write() = Some(modes);
}

/// Every project's value of `key`, for the mirrors.
fn project_rows(conn: &Connection, key: &str) -> Vec<(String, String)> {
    let Ok(mut stmt) =
        conn.prepare("SELECT project_id, value FROM project_settings WHERE key = ?1")
    else {
        return Vec::new();
    };
    stmt.query_map([key], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// A whole number of MiB, at least [`MIN_MEMORY_MIB`]; `None` when absent or
/// blank.
pub fn parse_memory(raw: Option<&str>) -> Result<u32> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DEFAULT_MEMORY_MIB);
    };
    match raw.parse::<u32>() {
        Ok(mib) if mib >= MIN_MEMORY_MIB => Ok(mib),
        _ => Err(Error::Other(format!(
            "{MEMORY_SETTING} must be a whole number of MiB, at least {MIN_MEMORY_MIB}; got {raw:?}"
        ))),
    }
}

pub fn parse_cpus(raw: Option<&str>) -> Result<u8> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DEFAULT_CPUS);
    };
    match raw.parse::<u8>() {
        Ok(cpus) if (1..=32).contains(&cpus) => Ok(cpus),
        _ => Err(Error::Other(format!(
            "{CPUS_SETTING} must be between 1 and 32; got {raw:?}"
        ))),
    }
}

/// A whole number of GiB between [`MIN_DISK_GIB`] and [`MAX_DISK_GIB`].
pub fn parse_disk(raw: Option<&str>) -> Result<u32> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(DEFAULT_DISK_GIB);
    };
    match raw.parse::<u32>() {
        Ok(gib) if (MIN_DISK_GIB..=MAX_DISK_GIB).contains(&gib) => Ok(gib),
        _ => Err(Error::Other(format!(
            "{DISK_SETTING} must be a whole number of GiB between {MIN_DISK_GIB} and \
             {MAX_DISK_GIB}; got {raw:?}"
        ))),
    }
}

/// A `http(s)://` or `file://` base URL, trimmed; blank is the release URL.
pub fn parse_image_url(raw: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if ["https://", "http://", "file://"]
        .iter()
        .any(|scheme| raw.starts_with(scheme))
    {
        Ok(Some(raw.trim_end_matches('/').to_string()))
    } else {
        Err(Error::Other(format!(
            "{IMAGE_URL_SETTING} must start with https://, http:// or file://; got {raw:?}"
        )))
    }
}

/// Minutes, `0` for never.
pub fn parse_idle_stop(raw: Option<&str>) -> Result<Option<Duration>> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Some(Duration::from_secs(DEFAULT_IDLE_STOP_MINUTES * 60)));
    };
    match raw.parse::<u64>() {
        Ok(0) => Ok(None),
        Ok(minutes) if minutes <= 24 * 60 => Ok(Some(Duration::from_secs(minutes * 60))),
        _ => Err(Error::Other(format!(
            "{IDLE_STOP_SETTING} must be a number of minutes (0 = never, at most 1440); got {raw:?}"
        ))),
    }
}

/// Validate `value` for `key` and return it normalized for storage, or why it
/// is refused. `None` for a key that is not a box setting.
pub fn validate(key: &str, value: &str) -> Option<Result<String>> {
    Some(match key {
        MEMORY_SETTING => parse_memory(Some(value)).map(|v| v.to_string()),
        CPUS_SETTING => parse_cpus(Some(value)).map(|v| v.to_string()),
        DISK_SETTING => parse_disk(Some(value)).map(|v| v.to_string()),
        IMAGE_URL_SETTING => parse_image_url(Some(value)).map(Option::unwrap_or_default),
        IDLE_STOP_SETTING => {
            parse_idle_stop(Some(value)).map(|d| d.map_or(0, |d| d.as_secs() / 60).to_string())
        }
        EGRESS_MODE_SETTING => EgressMode::parse(Some(value)).map(|m| m.as_setting().to_string()),
        _ => return None,
    })
}

/// The stored `box.egress_allow` value: a JSON array of host patterns. Anything
/// else reads as no additions.
pub fn parse_egress_allow(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw)
        .map(|patterns| {
            patterns
                .into_iter()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A project's egress additions, from the mirror.
pub fn egress_allow(project_id: &str) -> Vec<String> {
    EGRESS
        .read()
        .as_ref()
        .and_then(|m| m.get(project_id).cloned())
        .unwrap_or_default()
}

/// The user's global egress changes, from the mirror: additions only. A
/// template entry turned off globally (`remove`) has no control any more and
/// is ignored; the next write clears it. A project's `egress.disable` is the
/// route that remains.
pub fn global_egress() -> super::allowlist::Delta {
    GLOBAL_EGRESS.read().clone().unwrap_or_default()
}

/// Replace the global egress changes in the mirror (the setter persists),
/// keeping the additions only.
pub fn set_global_egress(delta: super::allowlist::Delta) {
    *GLOBAL_EGRESS.write() = Some(super::allowlist::Delta {
        add: delta.add,
        remove: Vec::new(),
    });
}

/// The posture `project_id`'s boxes answer an unlisted host with: the
/// project's override, else the global setting.
pub fn egress_mode(project_id: &str) -> EgressMode {
    EGRESS_MODES
        .read()
        .as_ref()
        .and_then(|m| m.get(project_id).copied())
        .unwrap_or_else(|| current().egress_mode)
}

/// Replace a project's posture override in the mirror (the setter persists);
/// `None` is back to the global.
pub fn set_egress_mode_override(project_id: &str, mode: Option<EgressMode>) {
    let mut modes = EGRESS_MODES.write();
    let modes = modes.get_or_insert_with(HashMap::new);
    match mode {
        Some(mode) => {
            modes.insert(project_id.to_string(), mode);
        }
        None => {
            modes.remove(project_id);
        }
    }
}

/// Replace a project's egress additions in the mirror (the setter persists).
pub fn set_egress_allow(project_id: &str, patterns: Vec<String>) {
    EGRESS
        .write()
        .get_or_insert_with(HashMap::new)
        .insert(project_id.to_string(), patterns);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knobs_parse_with_their_floors_and_defaults() {
        assert_eq!(parse_memory(None).unwrap(), DEFAULT_MEMORY_MIB);
        assert_eq!(parse_memory(Some(" 4096 ")).unwrap(), 4096);
        assert!(parse_memory(Some("256")).is_err());
        assert!(parse_memory(Some("lots")).is_err());
        assert_eq!(parse_cpus(None).unwrap(), DEFAULT_CPUS);
        assert_eq!(parse_cpus(Some("4")).unwrap(), 4);
        assert!(parse_cpus(Some("0")).is_err());
        assert_eq!(parse_image_url(Some("")).unwrap(), None);
        assert_eq!(
            parse_image_url(Some("file:///tmp/out/"))
                .unwrap()
                .as_deref(),
            Some("file:///tmp/out")
        );
        assert!(parse_image_url(Some("ftp://x")).is_err());
        assert_eq!(parse_idle_stop(Some("0")).unwrap(), None);
        assert_eq!(
            parse_idle_stop(Some("3")).unwrap(),
            Some(Duration::from_secs(180))
        );
        assert!(parse_idle_stop(Some("-1")).is_err());
        assert_eq!(parse_disk(None).unwrap(), 8);
        assert_eq!(parse_disk(Some(" 64 ")).unwrap(), 64);
        for bad in ["3", "257", "1.5", "lots"] {
            assert!(parse_disk(Some(bad)).is_err(), "{bad}");
        }
        for retired in ["boxes_enabled", "box_layers"] {
            assert!(
                validate(retired, "true").is_none(),
                "{retired} is no longer a box setting"
            );
        }
        assert_eq!(
            validate("box_idle_stop_minutes", "0").unwrap().unwrap(),
            "0"
        );
        assert!(validate("docker_image", "x").is_none());
    }

    /// The posture spells three ways, blank is open, and a project's override
    /// wins over the global for that project alone.
    #[test]
    fn the_egress_mode_parses_and_a_project_overrides_it() {
        let _serial = TEST_LOCK.blocking_lock();
        assert_eq!(EgressMode::parse(None).unwrap(), EgressMode::Open);
        assert_eq!(EgressMode::parse(Some(" ask ")).unwrap(), EgressMode::Ask);
        assert_eq!(
            EgressMode::parse(Some("allowlist")).unwrap(),
            EgressMode::AllowList
        );
        assert!(EgressMode::parse(Some("yolo")).is_err());
        assert_eq!(
            validate(EGRESS_MODE_SETTING, "ask").unwrap().unwrap(),
            "ask"
        );
        assert!(validate(EGRESS_MODE_SETTING, "never").unwrap().is_err());
        assert_eq!(EgressMode::Ask.proxy_mode(), fletch_box::EgressMode::Hold);
        assert_eq!(
            EgressMode::AllowList.proxy_mode(),
            fletch_box::EgressMode::Refuse
        );

        set(BoxSettings {
            egress_mode: EgressMode::Ask,
            ..BoxSettings::default()
        });
        assert_eq!(egress_mode("p-mode"), EgressMode::Ask);
        set_egress_mode_override("p-mode", Some(EgressMode::AllowList));
        assert_eq!(egress_mode("p-mode"), EgressMode::AllowList);
        assert_eq!(egress_mode("p-other"), EgressMode::Ask);
        set_egress_mode_override("p-mode", None);
        assert_eq!(egress_mode("p-mode"), EgressMode::Ask);
        set(BoxSettings::default());
    }

    /// A global `remove` is read as nothing: the control that wrote it is gone.
    #[test]
    fn the_global_turn_offs_are_ignored() {
        let _serial = TEST_LOCK.blocking_lock();
        set_global_egress(super::super::allowlist::Delta {
            add: vec!["a.test".into()],
            remove: vec!["rubygems.org".into()],
        });
        let global = global_egress();
        assert_eq!(global.add, vec!["a.test".to_string()]);
        assert!(global.remove.is_empty());
        set_global_egress(super::super::allowlist::Delta::default());
    }

    #[test]
    fn egress_additions_are_a_json_array_of_patterns() {
        assert_eq!(
            parse_egress_allow(r#"["example.com", " *.corp.test ", ""]"#),
            vec!["example.com".to_string(), "*.corp.test".to_string()]
        );
        assert!(parse_egress_allow("not json").is_empty());
        set_egress_allow("p-settings-test", vec!["a.test".into()]);
        assert_eq!(egress_allow("p-settings-test"), vec!["a.test".to_string()]);
        assert!(egress_allow("p-unknown").is_empty());
    }

    #[test]
    fn load_reads_the_table_with_defaults_for_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::database::init(dir.path()).unwrap();
        let conn = db.lock();
        assert_eq!(load(&conn), BoxSettings::default());
        database::set_setting(&conn, MEMORY_SETTING, "3072").unwrap();
        database::set_setting(&conn, IDLE_STOP_SETTING, "0").unwrap();
        database::set_setting(&conn, CPUS_SETTING, "bogus").unwrap();
        database::set_setting(&conn, EGRESS_MODE_SETTING, "allowlist").unwrap();
        let loaded = load(&conn);
        assert_eq!(loaded.memory_mib, 3072);
        assert_eq!(loaded.idle_stop, None);
        assert_eq!(loaded.cpus, DEFAULT_CPUS, "a bad row reads as the default");
        assert_eq!(loaded.egress_mode, EgressMode::AllowList);
    }
}
