//! The box engine's settings (the `boxes_enabled` switch and the `box_*`
//! knobs), mirrored in-process the way the container engines' launch knobs
//! are (`sandbox::docker::settings`):
//! the spawn path and the registry have no DB handle. Seeded at boot and kept
//! in sync by `commands::set_box_setting_impl` / `box_egress_allow_impl`.

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::RwLock;
use rusqlite::Connection;

use crate::database;
use crate::error::{Error, Result};

/// The switch: "use isolated VMs when available". Opt-out — absent is on, and
/// only an explicit `false` turns every box code path off. On a machine that
/// cannot boot a box the switch changes nothing: the runtime probe refuses
/// first, and the automatic engine choice lands on seatbelt.
pub const ENABLED_SETTING: &str = "boxes_enabled";
/// Guest RAM cap per box, MiB.
pub const MEMORY_SETTING: &str = "box_memory_mib";
/// vCPUs per box.
pub const CPUS_SETTING: &str = "box_cpus";
/// Alternate base URL for the image download; empty is the GitHub release.
pub const IMAGE_URL_SETTING: &str = "box_image_url";
/// Minutes a box with no attached launch stays up; `0` is never.
pub const IDLE_STOP_SETTING: &str = "box_idle_stop_minutes";
/// The image layers attached to every box besides the base, comma-separated
/// (`fletch_box::image_store::OPTIONAL_LAYERS`); blank is the default set.
pub const LAYERS_SETTING: &str = "box_layers";
/// Project setting: a JSON array of host patterns added to the default egress
/// allow-list.
pub const EGRESS_ALLOW_KEY: &str = "box.egress_allow";

/// Every global key, for the host-owned allowlist.
pub const KEYS: &[&str] = &[
    ENABLED_SETTING,
    MEMORY_SETTING,
    CPUS_SETTING,
    IMAGE_URL_SETTING,
    IDLE_STOP_SETTING,
    LAYERS_SETTING,
];

pub const DEFAULT_MEMORY_MIB: u32 = 2048;
pub const MIN_MEMORY_MIB: u32 = 512;
pub const DEFAULT_CPUS: u8 = 2;
pub const DEFAULT_IDLE_STOP_MINUTES: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSettings {
    pub enabled: bool,
    pub memory_mib: u32,
    pub cpus: u8,
    /// `None` is the GitHub release for this build's pin.
    pub image_url: Option<String>,
    /// `None` is never.
    pub idle_stop: Option<Duration>,
    /// The image layers every box gets besides the base, in
    /// `OPTIONAL_LAYERS` order; the install fetches them, the boot attaches them.
    pub layers: Vec<String>,
}

impl Default for BoxSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_mib: DEFAULT_MEMORY_MIB,
            cpus: DEFAULT_CPUS,
            image_url: None,
            idle_stop: Some(Duration::from_secs(DEFAULT_IDLE_STOP_MINUTES * 60)),
            layers: default_layers(),
        }
    }
}

static SETTINGS: RwLock<BoxSettings> = RwLock::new(BoxSettings {
    enabled: true,
    memory_mib: DEFAULT_MEMORY_MIB,
    cpus: DEFAULT_CPUS,
    image_url: None,
    idle_stop: Some(Duration::from_secs(DEFAULT_IDLE_STOP_MINUTES * 60)),
    layers: Vec::new(),
});

/// The layers a host gets without a `box_layers` row.
pub fn default_layers() -> Vec<String> {
    fletch_box::image_store::DEFAULT_LAYERS
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// `project_id → patterns`, the per-project egress additions.
static EGRESS: RwLock<Option<HashMap<String, Vec<String>>>> = RwLock::new(None);

/// The user's global egress changes over the template
/// (`allowlist::GLOBAL_SETTING`).
static GLOBAL_EGRESS: RwLock<Option<super::allowlist::Delta>> = RwLock::new(None);

/// Held by every test that flips the flag or the selected engine: both are
/// process-wide, so such tests run one at a time. Async-aware so an async test
/// may hold it across its awaits; a sync test takes `blocking_lock`.
#[cfg(test)]
pub(crate) static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn current() -> BoxSettings {
    SETTINGS.read().clone()
}

pub fn enabled() -> bool {
    SETTINGS.read().enabled
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
        enabled: parse_enabled(raw(ENABLED_SETTING).as_deref()),
        memory_mib: parse_memory(raw(MEMORY_SETTING).as_deref()).unwrap_or(DEFAULT_MEMORY_MIB),
        cpus: parse_cpus(raw(CPUS_SETTING).as_deref()).unwrap_or(DEFAULT_CPUS),
        image_url: parse_image_url(raw(IMAGE_URL_SETTING).as_deref()).unwrap_or(None),
        idle_stop: parse_idle_stop(raw(IDLE_STOP_SETTING).as_deref())
            .unwrap_or(Some(Duration::from_secs(DEFAULT_IDLE_STOP_MINUTES * 60))),
        layers: parse_layers(raw(LAYERS_SETTING).as_deref()).unwrap_or_else(|_| default_layers()),
    }
}

/// Seed both mirrors from the database (boot).
pub fn seed(conn: &Connection) {
    set(load(conn));
    set_global_egress(super::allowlist::Delta::parse(
        database::get_setting(conn, super::allowlist::GLOBAL_SETTING).as_deref(),
    ));
    let mut map = HashMap::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT project_id, value FROM project_settings WHERE key = ?1")
    {
        let rows = stmt.query_map([EGRESS_ALLOW_KEY], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        if let Ok(rows) = rows {
            for (project_id, value) in rows.flatten() {
                map.insert(project_id, parse_egress_allow(&value));
            }
        }
    }
    *EGRESS.write() = Some(map);
}

/// Only an explicit `false` turns the engine off.
pub fn parse_enabled(raw: Option<&str>) -> bool {
    raw.map(str::trim) != Some("false")
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

/// Comma-separated layer names out of `OPTIONAL_LAYERS`, returned in that
/// order without repeats; blank is the default set, `none` is no layer.
pub fn parse_layers(raw: Option<&str>) -> Result<Vec<String>> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(default_layers());
    };
    if raw == "none" {
        return Ok(Vec::new());
    }
    let known = fletch_box::image_store::OPTIONAL_LAYERS;
    for name in raw.split(',').map(str::trim) {
        if !known.contains(&name) {
            return Err(Error::Other(format!(
                "{LAYERS_SETTING} must name layers out of {}, comma-separated, or `none`; got {raw:?}",
                known.join(", ")
            )));
        }
    }
    Ok(known
        .iter()
        .filter(|k| raw.split(',').any(|n| n.trim() == **k))
        .map(|k| k.to_string())
        .collect())
}

/// Validate `value` for `key` and return it normalized for storage, or why it
/// is refused. `None` for a key that is not a box setting.
pub fn validate(key: &str, value: &str) -> Option<Result<String>> {
    Some(match key {
        ENABLED_SETTING => Ok(parse_enabled(Some(value)).to_string()),
        MEMORY_SETTING => parse_memory(Some(value)).map(|v| v.to_string()),
        CPUS_SETTING => parse_cpus(Some(value)).map(|v| v.to_string()),
        IMAGE_URL_SETTING => parse_image_url(Some(value)).map(Option::unwrap_or_default),
        IDLE_STOP_SETTING => {
            parse_idle_stop(Some(value)).map(|d| d.map_or(0, |d| d.as_secs() / 60).to_string())
        }
        LAYERS_SETTING => parse_layers(Some(value)).map(|layers| {
            if layers.is_empty() {
                "none".to_string()
            } else {
                layers.join(",")
            }
        }),
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

/// The user's global egress changes, from the mirror.
pub fn global_egress() -> super::allowlist::Delta {
    GLOBAL_EGRESS.read().clone().unwrap_or_default()
}

/// Replace the global egress changes in the mirror (the setter persists).
pub fn set_global_egress(delta: super::allowlist::Delta) {
    *GLOBAL_EGRESS.write() = Some(delta);
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
    fn only_an_explicit_true_enables() {
        assert!(parse_enabled(Some("true")));
        for raw in [None, Some(""), Some("1"), Some("TRUE"), Some("yes")] {
            assert!(!parse_enabled(raw), "{raw:?}");
        }
    }

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
        assert_eq!(parse_layers(None).unwrap(), ["devtools"]);
        assert_eq!(parse_layers(Some(" ")).unwrap(), ["devtools"]);
        assert_eq!(
            parse_layers(Some("browser, devtools,browser")).unwrap(),
            ["devtools", "browser"],
            "canonical order, no repeats"
        );
        assert!(parse_layers(Some("none")).unwrap().is_empty());
        assert!(parse_layers(Some("gpu")).is_err());
        assert_eq!(
            validate("box_layers", "browser").unwrap().unwrap(),
            "browser"
        );
        assert_eq!(validate("box_layers", "none").unwrap().unwrap(), "none");
        assert_eq!(
            validate("boxes_enabled", "false").unwrap().unwrap(),
            "false"
        );
        assert_eq!(validate("boxes_enabled", "nope").unwrap().unwrap(), "true");
        assert_eq!(
            validate("box_idle_stop_minutes", "0").unwrap().unwrap(),
            "0"
        );
        assert!(validate("docker_image", "x").is_none());
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
        assert!(load(&conn).enabled, "absent is on");
        database::set_setting(&conn, ENABLED_SETTING, "false").unwrap();
        database::set_setting(&conn, MEMORY_SETTING, "3072").unwrap();
        database::set_setting(&conn, IDLE_STOP_SETTING, "0").unwrap();
        database::set_setting(&conn, CPUS_SETTING, "bogus").unwrap();
        let loaded = load(&conn);
        assert!(!loaded.enabled);
        assert_eq!(loaded.memory_mib, 3072);
        assert_eq!(loaded.idle_stop, None);
        assert_eq!(loaded.cpus, DEFAULT_CPUS, "a bad row reads as the default");
    }
}
