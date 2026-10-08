use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use serde_json::json;

use super::*;

const MIN: i64 = 60 * 1000;
const MARGIN: i64 = 30 * MIN;

/// A refresh answer: the new access token, a rotated refresh token, and how
/// long the new access token lives.
#[derive(Debug, Clone)]
struct Grant {
    access: String,
    refresh: Option<String>,
    lives_ms: i64,
}

fn grant(access: &str, refresh: Option<&str>) -> Grant {
    Grant {
        access: access.into(),
        refresh: refresh.map(Into::into),
        lives_ms: 8 * 60 * MIN,
    }
}

type Hook<const RECHECK: bool> = Box<dyn Fn(&Fake<RECHECK>) + Send + Sync>;

/// An in-memory store and a scripted sign-in server. `RECHECK` picks the
/// rule after a refusal of a token another process rotated.
struct Fake<const RECHECK: bool> {
    key: String,
    stored: Mutex<Option<String>>,
    version: AtomicU64,
    fail_saves: AtomicBool,
    fits: AtomicBool,
    saves: AtomicUsize,
    answers: Mutex<VecDeque<Result<Grant, RefreshFailure>>>,
    sent: Mutex<Vec<String>>,
    /// Runs while a refresh request is out, before its answer.
    during_refresh: Mutex<Option<Hook<RECHECK>>>,
    mark_path: Option<PathBuf>,
}

fn unique_key() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("fake-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn login(access: &str, refresh: Option<&str>, expires_ms: i64) -> Value {
    json!({ "access": access, "refresh": refresh, "expires": expires_ms, "kept": "as is" })
}

impl<const RECHECK: bool> Fake<RECHECK> {
    fn holding(json: Option<Value>) -> Self {
        Self {
            key: unique_key(),
            stored: Mutex::new(json.map(|j| j.to_string())),
            version: AtomicU64::new(1),
            fail_saves: AtomicBool::new(false),
            fits: AtomicBool::new(true),
            saves: AtomicUsize::new(0),
            answers: Mutex::new(VecDeque::new()),
            sent: Mutex::new(Vec::new()),
            during_refresh: Mutex::new(None),
            mark_path: None,
        }
    }

    fn answer(&self, answer: Result<Grant, RefreshFailure>) {
        self.answers.lock().push_back(answer);
    }

    /// What a new sign-in (or another process) does: replace the stored login.
    fn store(&self, json: Value) {
        *self.stored.lock() = Some(json.to_string());
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    fn stored_json(&self) -> Value {
        serde_json::from_str(self.stored.lock().as_ref().unwrap()).unwrap()
    }
}

impl<const RECHECK: bool> LoginProvider for Fake<RECHECK> {
    type Place = String;
    type Grant = Grant;
    type Launch = String;
    const PROVIDER: &'static str = "fake";
    const AFTER_ROTATION: AfterRotation = if RECHECK {
        AfterRotation::Recheck
    } else {
        AfterRotation::UseIfUnexpired
    };

    fn key(&self) -> String {
        self.key.clone()
    }

    fn stamp(&self) -> Option<String> {
        self.stored
            .lock()
            .as_ref()
            .map(|_| self.version.load(Ordering::SeqCst).to_string())
    }

    fn load(&self) -> Result<Option<(String, String)>, String> {
        Ok(self.stored.lock().clone().map(|j| (j, "place".to_string())))
    }

    fn parse(&self, json: &Value) -> Option<Creds> {
        json.get("access")?.as_str().filter(|a| !a.is_empty())?;
        Some(Creds {
            refresh: json
                .get("refresh")
                .and_then(Value::as_str)
                .map(str::to_string),
            expires_at_ms: json.get("expires").and_then(Value::as_i64),
        })
    }

    fn due(&self, creds: &Creds, _json: &Value, now_ms: i64) -> bool {
        creds.expires_at_ms.is_some_and(|e| e - now_ms <= MARGIN)
    }

    fn fits(&self, place: &String, _len: usize) -> bool {
        assert_eq!(place, "place");
        self.fits.load(Ordering::SeqCst)
    }

    fn refresh(&self, refresh_token: &str) -> Result<Grant, RefreshFailure> {
        self.sent.lock().push(refresh_token.to_string());
        let hook = self.during_refresh.lock().take();
        if let Some(hook) = hook {
            hook(self);
        }
        self.answers
            .lock()
            .pop_front()
            .unwrap_or_else(|| Err(RefreshFailure::Failed("no scripted answer".into())))
    }

    fn apply(&self, json: &mut Value, grant: &Grant, now_ms: i64) -> bool {
        let Some(obj) = json.as_object_mut() else {
            return false;
        };
        obj.insert("access".into(), grant.access.clone().into());
        obj.insert("expires".into(), (now_ms + grant.lives_ms).into());
        if let Some(refresh) = &grant.refresh {
            obj.insert("refresh".into(), refresh.clone().into());
        }
        true
    }

    fn save(&self, place: &String, json: &Value) -> Result<(), String> {
        assert_eq!(place, "place");
        if self.fail_saves.load(Ordering::SeqCst) {
            return Err("store refused".into());
        }
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.store(json.clone());
        Ok(())
    }

    fn launch(&self, json: &Value, _creds: &Creds) -> String {
        json["access"].as_str().unwrap().to_string()
    }

    fn mark_of(&self, stamp: Option<&str>, _json: &Value) -> Option<String> {
        stamp.map(str::to_string)
    }

    fn current_mark(&self) -> Option<String> {
        self.stamp()
    }

    fn mark_path(&self) -> Option<PathBuf> {
        self.mark_path.clone()
    }
}

fn engine(fake: Fake<false>) -> HostLogin<Fake<false>> {
    HostLogin::new(fake)
}

fn soon() -> i64 {
    now_ms() + 10 * MIN
}

fn later() -> i64 {
    now_ms() + 5 * 60 * MIN
}

#[test]
fn a_fresh_login_is_used_without_a_refresh() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), later()))));
    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-1");
    assert!(e.provider().sent.lock().is_empty());
}

#[test]
fn a_due_login_is_refreshed_and_the_rotated_login_stored_with_its_other_fields() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider().answer(Ok(grant("at-2", Some("rt-2"))));

    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-2");

    let stored = e.provider().stored_json();
    assert_eq!(stored["refresh"], "rt-2");
    assert_eq!(stored["kept"], "as is");
    assert_eq!(*e.provider().sent.lock(), vec!["rt-1"]);
}

#[test]
fn nothing_stored_is_signed_out() {
    let e = engine(Fake::holding(None));
    assert_eq!(e.credential(Demand::Launch), Err(LoginError::SignedOut));
}

#[test]
fn a_login_without_a_refresh_token_launches_until_it_expires() {
    let e = engine(Fake::holding(Some(login("at-1", None, soon()))));
    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-1");
    e.provider().store(login("at-1", None, now_ms() - 1));
    assert_eq!(e.credential(Demand::Launch), Err(LoginError::Revoked));
}

#[test]
fn a_refused_refresh_reads_as_revoked_and_sticks_until_the_login_changes() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider().answer(Err(RefreshFailure::Rejected));

    assert_eq!(e.credential(Demand::Launch), Err(LoginError::Revoked));
    assert!(e.is_revoked());
    assert_eq!(e.credential(Demand::Launch), Err(LoginError::Revoked));
    assert_eq!(e.provider().sent.lock().len(), 1, "no second refresh");

    e.provider().store(login("at-new", Some("rt-new"), later()));
    assert!(!e.is_revoked());
    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-new");
}

#[test]
fn a_refused_refresh_mark_recorded_to_a_file_outlives_the_engine() {
    let td = tempfile::tempdir().unwrap();
    let mut fake = Fake::holding(Some(login("at-1", Some("rt-1"), soon())));
    fake.mark_path = Some(td.path().join("marks").join("fake"));
    let key = fake.key.clone();
    fake.answer(Err(RefreshFailure::Rejected));
    let e = engine(fake);
    assert_eq!(e.credential(Demand::Launch), Err(LoginError::Revoked));
    assert!(td.path().join("marks/fake").is_file());
    REVOKED
        .lock()
        .as_mut()
        .map(|m| m.remove(&format!("fake:{key}")));

    assert!(e.is_revoked());
}

#[test]
fn a_refusal_after_another_process_rotated_the_login_uses_its_login() {
    let fake = Fake::holding(Some(login("at-1", Some("rt-1"), soon())));
    *fake.during_refresh.lock() = Some(Box::new(|f| {
        f.store(login("at-other", Some("rt-other"), later()))
    }));
    fake.answer(Err(RefreshFailure::Rejected));
    let e = engine(fake);

    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-other");
    assert!(!e.is_revoked());
    assert_eq!(e.provider().sent.lock().len(), 1);
}

#[test]
fn a_refusal_after_a_rotation_rechecks_and_refreshes_the_rotated_login_once() {
    let fake: Fake<true> = Fake::holding(Some(login("at-1", Some("rt-1"), soon())));
    *fake.during_refresh.lock() = Some(Box::new(|f| {
        f.store(login("at-other", Some("rt-other"), soon()))
    }));
    fake.answer(Err(RefreshFailure::Rejected));
    fake.answer(Ok(grant("at-3", Some("rt-3"))));
    let e = HostLogin::new(fake);

    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-3");

    assert_eq!(*e.provider().sent.lock(), vec!["rt-1", "rt-other"]);
    assert_eq!(e.provider().stored_json()["refresh"], "rt-3");
}

#[test]
fn a_rotation_that_is_not_due_is_launched_on_after_a_recheck() {
    let fake: Fake<true> = Fake::holding(Some(login("at-1", Some("rt-1"), soon())));
    *fake.during_refresh.lock() = Some(Box::new(|f| {
        f.store(login("at-other", Some("rt-other"), later()))
    }));
    fake.answer(Err(RefreshFailure::Rejected));
    let e = HostLogin::new(fake);

    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-other");
    assert_eq!(e.provider().sent.lock().len(), 1);
}

#[test]
fn a_second_refusal_after_a_recheck_is_a_revocation() {
    let fake: Fake<true> = Fake::holding(Some(login("at-1", Some("rt-1"), soon())));
    *fake.during_refresh.lock() = Some(Box::new(|f| {
        f.store(login("at-other", Some("rt-other"), soon()))
    }));
    fake.answer(Err(RefreshFailure::Rejected));
    fake.answer(Err(RefreshFailure::Rejected));
    let e = HostLogin::new(fake);

    assert_eq!(e.credential(Demand::Launch), Err(LoginError::Revoked));
    assert_eq!(e.provider().sent.lock().len(), 2);
}

#[test]
fn a_failed_refresh_launches_on_an_unexpired_login() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider()
        .answer(Err(RefreshFailure::Failed("offline".into())));
    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-1");
    assert_eq!(e.provider().stored_json()["refresh"], "rt-1");
}

#[test]
fn a_failed_refresh_of_an_expired_login_says_why() {
    let e = engine(Fake::holding(Some(login(
        "at-1",
        Some("rt-1"),
        now_ms() - 1,
    ))));
    e.provider()
        .answer(Err(RefreshFailure::Failed("offline".into())));
    assert_eq!(
        e.credential(Demand::Launch),
        Err(LoginError::Unavailable("offline".into()))
    );
}

#[test]
fn concurrent_credentials_refresh_once() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider().answer(Ok(grant("at-2", Some("rt-2"))));
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| assert_eq!(e.credential(Demand::Launch).unwrap(), "at-2"));
        }
    });
    assert_eq!(e.provider().sent.lock().len(), 1);
}

#[test]
fn replacing_a_rejected_token_forces_a_refresh_of_a_fresh_looking_one() {
    let expires = later();
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), expires))));
    e.provider().answer(Ok(grant("at-2", Some("rt-2"))));
    let replaced = e.credential(Demand::Replace {
        rejected_expires_at_ms: expires,
    });
    assert_eq!(replaced.unwrap(), "at-2");
}

#[test]
fn replacing_a_rejected_token_takes_a_newer_stored_one_without_a_refresh() {
    let e = engine(Fake::holding(Some(login(
        "at-newer",
        Some("rt-1"),
        later(),
    ))));
    let replaced = e.credential(Demand::Replace {
        rejected_expires_at_ms: soon(),
    });
    assert_eq!(replaced.unwrap(), "at-newer");
    assert!(e.provider().sent.lock().is_empty());
}

#[test]
fn a_replacement_whose_refresh_fails_is_unavailable_even_while_unexpired() {
    let expires = later();
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), expires))));
    e.provider()
        .answer(Err(RefreshFailure::Failed("offline".into())));
    let replaced = e.credential(Demand::Replace {
        rejected_expires_at_ms: expires,
    });
    assert_eq!(replaced, Err(LoginError::Unavailable("offline".into())));
}

#[test]
fn a_login_that_would_not_fit_back_is_not_refreshed() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider().fits.store(false, Ordering::SeqCst);
    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-1");
    assert!(e.provider().sent.lock().is_empty());
}

#[test]
fn an_expired_login_that_would_not_fit_back_says_why() {
    let e = engine(Fake::holding(Some(login(
        "at-1",
        Some("rt-1"),
        now_ms() - 1,
    ))));
    e.provider().fits.store(false, Ordering::SeqCst);
    let Err(LoginError::Unavailable(why)) = e.credential(Demand::Launch) else {
        panic!("expected unavailable");
    };
    assert!(why.contains("too large"), "{why}");
}

#[test]
fn a_rotation_the_store_refuses_is_kept_and_used_by_the_next_credential() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider().fail_saves.store(true, Ordering::SeqCst);
    e.provider().answer(Ok(grant("at-2", Some("rt-2"))));

    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-2");
    assert_eq!(e.provider().stored_json()["refresh"], "rt-1");
    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-2");
    assert_eq!(
        e.provider().sent.lock().len(),
        1,
        "the spent token isn't sent again"
    );
}

#[test]
fn a_kept_rotation_is_saved_once_the_store_takes_it() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider().fail_saves.store(true, Ordering::SeqCst);
    e.provider().answer(Ok(grant("at-2", Some("rt-2"))));
    e.credential(Demand::Launch).unwrap();
    e.provider().fail_saves.store(false, Ordering::SeqCst);

    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-2");

    assert_eq!(e.provider().stored_json()["refresh"], "rt-2");
    assert_eq!(e.provider().saves.load(Ordering::SeqCst), 1);
    e.credential(Demand::Launch).unwrap();
    assert_eq!(e.provider().saves.load(Ordering::SeqCst), 1);
}

#[test]
fn a_sign_in_after_a_refused_save_wins_over_the_kept_rotation() {
    let e = engine(Fake::holding(Some(login("at-1", Some("rt-1"), soon()))));
    e.provider().fail_saves.store(true, Ordering::SeqCst);
    e.provider().answer(Ok(grant("at-2", Some("rt-2"))));
    e.credential(Demand::Launch).unwrap();
    e.provider().fail_saves.store(false, Ordering::SeqCst);
    e.provider()
        .store(login("at-signed-in", Some("rt-signed-in"), later()));

    assert_eq!(e.credential(Demand::Launch).unwrap(), "at-signed-in");
    assert_eq!(e.provider().stored_json()["refresh"], "rt-signed-in");
}

#[test]
fn a_kept_value_lasts_while_the_stamp_holds_and_is_dropped_when_it_moves() {
    let kept: Kept<u32> = Kept::new();
    kept.keep("a", 7, Some("s1".into()));
    assert_eq!(kept.current("a", Some("s1")), Some(7));
    assert_eq!(kept.current("a", Some("s2")), None);
    assert_eq!(kept.current("a", Some("s1")), None);
}

#[test]
fn one_key_gets_one_lock_and_another_key_another() {
    static FLIGHTS: Flights<Mutex<()>> = Flights::new();
    assert!(Arc::ptr_eq(&FLIGHTS.get("a"), &FLIGHTS.get("a")));
    assert!(!Arc::ptr_eq(&FLIGHTS.get("a"), &FLIGHTS.get("b")));
}
