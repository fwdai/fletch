//! The agent's view of the Run panel, plus a way to get the user's
//! attention: `run_start`, `run_stop`, `run_status`, `run_log` and `notify`.
//!
//! Sixteen sessions in the transcripts said the agent could not launch the
//! app to see its change, and six reached for `say` or `afplay` to signal
//! that it was done. The run ops drive the very process the ▶ button starts
//! — same command, same sandbox, same log — so what the agent sees is what
//! the panel shows. `notify` relays a title and body as an event; the
//! desktop turns it into an OS notification under the user's mute.
//!
//! Wraps the agent's other dispatcher: every op not named here falls through
//! unchanged. None of these publish anything, so none is caps-gated.

use std::sync::Arc;

use serde_json::{json, Value};

use super::{Response, RpcDispatcher, RpcEvent, RpcFuture};
use crate::error::Result;
use crate::run_session::{RunPhase, RunStateSnapshot};

/// Emitted by `notify`: payload `{ "title", "body" }`. The watcher relays it
/// to the clients as `agent:notify`.
pub const EVENT_NOTIFY: &str = "agent.notify";

const LOG_DEFAULT_LINES: usize = 200;
const LOG_MAX_LINES: usize = 2000;
/// A notification is a headline, not a report.
const MAX_NOTIFY_TITLE_CHARS: usize = 120;
const MAX_NOTIFY_BODY_CHARS: usize = 500;

/// What the dispatcher needs from the host: the Run panel's process for one
/// agent. The supervisor implements it; tests use a fake.
pub trait RunControl: Send + Sync {
    /// Start the agent's run (setup first when it never completed). A no-op
    /// when one is already active, and when the project has nothing to run —
    /// the snapshot's phase tells those apart.
    fn start(&self, agent_id: &str) -> Result<()>;
    /// Kill the active run. Idempotent.
    fn stop(&self, agent_id: &str) -> Result<()>;
    fn state(&self, agent_id: &str) -> RunStateSnapshot;
}

fn is_run_op(op: &str) -> bool {
    matches!(
        op,
        "run_start" | "run_stop" | "run_status" | "run_log" | "notify"
    )
}

pub struct RunDispatcher {
    inner: Arc<dyn RpcDispatcher>,
    control: Arc<dyn RunControl>,
    agent_id: String,
}

impl RunDispatcher {
    pub fn new(
        control: Arc<dyn RunControl>,
        agent_id: &str,
        inner: Arc<dyn RpcDispatcher>,
    ) -> Self {
        Self {
            inner,
            control,
            agent_id: agent_id.to_string(),
        }
    }

    fn run_start(&self, id: &str) -> Response {
        let before = self.control.state(&self.agent_id);
        let already_running = matches!(before.phase, RunPhase::Setup | RunPhase::Running);
        if let Err(e) = self.control.start(&self.agent_id) {
            return Response::err(id, format!("run_start: {e}"));
        }
        let after = self.control.state(&self.agent_id);
        // `start` answers Ok and touches nothing when the project has no run
        // command; the agent needs to hear that, not "started". A command
        // that ran is told apart from that by its output: the host writes
        // the `$ cmd` header the moment a phase begins, so even a command
        // that finished before this second look advanced the log.
        let started = already_running
            || matches!(after.phase, RunPhase::Setup | RunPhase::Running)
            || after.log_seq > before.log_seq;
        if !started {
            return Response::err(
                id,
                "run_start: nothing to run — the project has no run command; the user sets one \
                 in Project Settings › Run",
            );
        }
        let exited = matches!(after.phase, RunPhase::Stopped);
        json_ok(
            id,
            "run_start",
            &json!({
                "phase": after.phase,
                "port": after.port,
                "already_running": already_running,
                "exited": exited,
                "last_error": after.last_error,
                "note": if exited {
                    "the command already finished; run_log has its output"
                } else {
                    "run_status reports the phase and port; the run phase follows setup \
                     when setup had never completed"
                },
            }),
        )
    }

    fn run_stop(&self, id: &str) -> Response {
        if let Err(e) = self.control.stop(&self.agent_id) {
            return Response::err(id, format!("run_stop: {e}"));
        }
        let after = self.control.state(&self.agent_id);
        json_ok(id, "run_stop", &json!({ "phase": after.phase }))
    }

    fn run_status(&self, id: &str) -> Response {
        let state = self.control.state(&self.agent_id);
        json_ok(
            id,
            "run_status",
            &json!({
                "phase": state.phase,
                "port": state.port,
                "last_error": state.last_error,
                "log_bytes": state.log.len(),
            }),
        )
    }

    fn run_log(&self, id: &str, args: &Value) -> Response {
        let lines = match positive_usize_arg(args, "lines") {
            Ok(n) => n.unwrap_or(LOG_DEFAULT_LINES).min(LOG_MAX_LINES),
            Err(e) => return Response::err(id, format!("run_log: {e}")),
        };
        let raw = args.get("raw").and_then(Value::as_bool).unwrap_or(false);
        let state = self.control.state(&self.agent_id);
        let text = String::from_utf8_lossy(&state.log);
        let (tail, truncated) = tail_lines(&text, lines, raw);
        json_ok(
            id,
            "run_log",
            &json!({
                "phase": state.phase,
                "port": state.port,
                "last_error": state.last_error,
                "truncated": truncated,
                "log": tail,
            }),
        )
    }

    fn notify(&self, id: &str, args: &Value) -> (Response, Vec<RpcEvent>) {
        let title = text_arg(args, "title");
        if title.is_empty() {
            return (
                Response::err(id, "notify requires a non-empty `title`"),
                Vec::new(),
            );
        }
        if title.chars().count() > MAX_NOTIFY_TITLE_CHARS {
            return (
                Response::err(
                    id,
                    format!(
                        "notify: `title` exceeds {MAX_NOTIFY_TITLE_CHARS} characters; put the \
                         detail in `body`"
                    ),
                ),
                Vec::new(),
            );
        }
        let body = text_arg(args, "body");
        if body.chars().count() > MAX_NOTIFY_BODY_CHARS {
            return (
                Response::err(
                    id,
                    format!(
                        "notify: `body` exceeds {MAX_NOTIFY_BODY_CHARS} characters; a \
                         notification is a headline, say the rest in your reply"
                    ),
                ),
                Vec::new(),
            );
        }
        (
            Response::ok(id, 0, "notified".to_string(), String::new()),
            vec![RpcEvent::named(
                EVENT_NOTIFY,
                json!({ "title": title, "body": body }),
            )],
        )
    }
}

impl RpcDispatcher for RunDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: &'a str,
        op: &'a str,
        args: &'a Value,
    ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
        if !is_run_op(op) {
            return self.inner.dispatch(id, op, args);
        }
        Box::pin(async move {
            match op {
                "run_start" => (self.run_start(id), Vec::new()),
                "run_stop" => (self.run_stop(id), Vec::new()),
                "run_status" => (self.run_status(id), Vec::new()),
                "run_log" => (self.run_log(id, args), Vec::new()),
                "notify" => self.notify(id, args),
                _ => unreachable!("is_run_op admitted {op}"),
            }
        })
    }
}

fn json_ok(id: &str, op: &str, value: &Value) -> Response {
    match serde_json::to_string_pretty(value) {
        Ok(json) => Response::ok(id, 0, json, String::new()),
        Err(e) => Response::err(id, format!("{op}: {e}")),
    }
}

/// A string arg, trimmed and whitespace-collapsed; empty when absent or not
/// a string.
fn text_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

fn positive_usize_arg(args: &Value, key: &str) -> std::result::Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0)
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a positive integer, got {v}")),
    }
}

/// The last `lines` lines of a PTY log as an agent reads them: escape
/// sequences stripped and carriage-return overwrites resolved (a progress
/// bar's final state, not every redraw), unless `raw`. Returns the tail and
/// whether anything was cut.
pub(crate) fn tail_lines(log: &str, lines: usize, raw: bool) -> (String, bool) {
    let lines = lines.clamp(1, LOG_MAX_LINES);
    let cleaned;
    let text = if raw {
        log
    } else {
        cleaned = plain_text(log);
        cleaned.as_str()
    };
    let all: Vec<&str> = text.lines().collect();
    let skip = all.len().saturating_sub(lines);
    (all[skip..].join("\n"), skip > 0)
}

/// A PTY transcript as plain lines: ANSI stripped, `\r\n` as a line break,
/// and a bare `\r` as the overwrite it is — what follows the last one on a
/// line replaces what came before. A `\r` with nothing after it yet (a
/// progress line caught mid-update) only moved the cursor, so the text
/// before it is what the terminal still shows.
fn plain_text(log: &str) -> String {
    let stripped = crate::sandbox::docker::setup_token::strip_ansi(log);
    stripped
        .replace("\r\n", "\n")
        .lines()
        .map(|line| {
            line.rsplit('\r')
                .find(|segment| !segment.is_empty())
                .unwrap_or("")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    /// A run the test scripts: `start` moves to `next_phase` with `port`,
    /// `stop` to Stopped; both count their calls.
    struct FakeRun {
        state: Mutex<RunStateSnapshot>,
        next_phase: RunPhase,
        port: Option<u16>,
        starts: Mutex<u32>,
        stops: Mutex<u32>,
    }

    impl FakeRun {
        fn new(next_phase: RunPhase, port: Option<u16>) -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(RunStateSnapshot {
                    phase: RunPhase::Idle,
                    last_error: None,
                    port: None,
                    log: Vec::new(),
                    log_seq: 0,
                }),
                next_phase,
                port,
                starts: Mutex::new(0),
                stops: Mutex::new(0),
            })
        }
    }

    impl RunControl for FakeRun {
        fn start(&self, _agent_id: &str) -> Result<()> {
            *self.starts.lock() += 1;
            let mut s = self.state.lock();
            if matches!(s.phase, RunPhase::Setup | RunPhase::Running) {
                return Ok(());
            }
            // Idle stands for "nothing to run": the host touches nothing.
            // Any other next phase is a command that began, header line
            // and all — Stopped being one that finished before the
            // dispatcher looked again.
            if !matches!(self.next_phase, RunPhase::Idle) {
                s.log.extend_from_slice(b"$ cmd\r\n");
                s.log_seq += 7;
            }
            s.phase = self.next_phase;
            s.port = self.port;
            Ok(())
        }
        fn stop(&self, _agent_id: &str) -> Result<()> {
            *self.stops.lock() += 1;
            let mut s = self.state.lock();
            s.phase = RunPhase::Stopped;
            s.port = None;
            Ok(())
        }
        fn state(&self, _agent_id: &str) -> RunStateSnapshot {
            self.state.lock().clone()
        }
    }

    struct Inner;

    impl RpcDispatcher for Inner {
        fn dispatch<'a>(
            &'a self,
            id: &'a str,
            op: &'a str,
            _args: &'a Value,
        ) -> RpcFuture<'a, (Response, Vec<RpcEvent>)> {
            Box::pin(async move {
                (
                    Response::ok(id, 0, format!("inner saw {op}"), String::new()),
                    Vec::new(),
                )
            })
        }
    }

    fn dispatcher(run: Arc<FakeRun>) -> RunDispatcher {
        RunDispatcher::new(run, "agent-1", Arc::new(Inner))
    }

    fn stdout(resp: &Response) -> Value {
        serde_json::from_str(resp.stdout.as_deref().unwrap_or("null")).unwrap()
    }

    #[tokio::test]
    async fn every_other_op_falls_through_to_the_inner_dispatcher() {
        let disp = dispatcher(FakeRun::new(RunPhase::Running, None));
        let (resp, fx) = disp.dispatch("i", "git_status", &Value::Null).await;
        assert_eq!(resp.stdout.as_deref(), Some("inner saw git_status"));
        assert!(fx.is_empty());
        for op in ["run_start", "run_stop", "run_status", "run_log", "notify"] {
            assert!(is_run_op(op), "{op}");
        }
        assert!(!is_run_op("git_push") && !is_run_op("set_title"));
    }

    #[tokio::test]
    async fn run_start_reports_the_phase_and_port_and_a_repeat_is_idempotent() {
        let run = FakeRun::new(RunPhase::Running, Some(3001));
        let disp = dispatcher(run.clone());
        let (resp, fx) = disp.dispatch("s", "run_start", &Value::Null).await;
        assert!(resp.ok, "{:?}", resp.error);
        let out = stdout(&resp);
        assert_eq!(out["phase"], "running");
        assert_eq!(out["port"], 3001);
        assert_eq!(out["already_running"], false);
        assert!(fx.is_empty());

        let (resp, _) = disp.dispatch("s", "run_start", &Value::Null).await;
        assert_eq!(stdout(&resp)["already_running"], true);
        assert_eq!(*run.starts.lock(), 2);

        let (resp, _) = disp.dispatch("t", "run_stop", &Value::Null).await;
        assert_eq!(stdout(&resp)["phase"], "stopped");
        assert_eq!(*run.stops.lock(), 1);
        let (resp, _) = disp.dispatch("u", "run_status", &Value::Null).await;
        let out = stdout(&resp);
        assert_eq!(out["phase"], "stopped");
        assert_eq!(out["port"], Value::Null);
    }

    /// `start` is a no-op for a project with no run command; the agent is
    /// told that, not "started".
    #[tokio::test]
    async fn run_start_with_nothing_to_run_says_so() {
        let disp = dispatcher(FakeRun::new(RunPhase::Idle, None));
        let (resp, _) = disp.dispatch("s", "run_start", &Value::Null).await;
        assert!(!resp.ok);
        assert!(
            resp.error.as_deref().unwrap().contains("nothing to run"),
            "{:?}",
            resp.error
        );
    }

    /// A short command can finish before the dispatcher looks again. Its
    /// output moved the log, so it ran — reported as exited, not as
    /// unconfigured.
    #[tokio::test]
    async fn a_command_that_finished_at_once_is_reported_as_exited_not_missing() {
        let run = FakeRun::new(RunPhase::Stopped, None);
        let disp = dispatcher(run.clone());
        let (resp, _) = disp.dispatch("s", "run_start", &Value::Null).await;
        assert!(resp.ok, "{:?}", resp.error);
        let out = stdout(&resp);
        assert_eq!(out["phase"], "stopped");
        assert_eq!(out["exited"], true);
        assert_eq!(out["already_running"], false);
        assert!(out["note"].as_str().unwrap().contains("run_log"));

        // Stopped before and stopped after, with no output: nothing ran.
        let idle = FakeRun::new(RunPhase::Idle, None);
        idle.state.lock().phase = RunPhase::Stopped;
        let (resp, _) = dispatcher(idle)
            .dispatch("s", "run_start", &Value::Null)
            .await;
        assert!(resp.error.as_deref().unwrap().contains("nothing to run"));
    }

    /// A progress line caught at its carriage return still shows its text;
    /// only what follows a `\r` on the same line overwrites it.
    #[test]
    fn a_bare_trailing_carriage_return_keeps_the_visible_text() {
        assert_eq!(plain_text("Progress 50%\r"), "Progress 50%");
        assert_eq!(plain_text("Progress 50%\rProgress 90%\r"), "Progress 90%");
        assert_eq!(plain_text("a\rb\r\nc"), "b\nc");
        assert_eq!(plain_text("\r"), "");
        let (tail, _) = tail_lines("line one\nProgress 50%\r", 5, false);
        assert_eq!(tail, "line one\nProgress 50%");
    }

    #[tokio::test]
    async fn run_log_tails_plain_text_and_validates_lines() {
        let run = FakeRun::new(RunPhase::Running, Some(5173));
        run.state.lock().port = Some(5173);
        run.state.lock().log = b"\x1b[2m$ pnpm dev\x1b[0m\r\n\
            downloading 10%\rdownloading 50%\rdownloading 100%\r\n\
            \x1b[32m  VITE v5  ready in 300 ms\x1b[0m\r\n\
            \x1b[31merror\x1b[0m: boom\r\n"
            .to_vec();
        let disp = dispatcher(run);
        let (resp, _) = disp.dispatch("l", "run_log", &json!({ "lines": 3 })).await;
        let out = stdout(&resp);
        assert_eq!(
            out["log"],
            "downloading 100%\n  VITE v5  ready in 300 ms\nerror: boom"
        );
        assert_eq!(out["truncated"], true);
        assert_eq!(out["port"], 5173);

        let (resp, _) = disp
            .dispatch("l", "run_log", &json!({ "lines": 1, "raw": true }))
            .await;
        assert_eq!(
            stdout(&resp)["log"],
            "\x1b[31merror\x1b[0m: boom",
            "raw keeps the escapes"
        );

        let (resp, _) = disp.dispatch("l", "run_log", &json!({ "lines": 0 })).await;
        assert!(resp
            .error
            .as_deref()
            .unwrap()
            .contains("`lines` must be a positive integer"));
    }

    #[tokio::test]
    async fn notify_validates_and_relays_a_trimmed_title_and_body() {
        let disp = dispatcher(FakeRun::new(RunPhase::Idle, None));
        for args in [json!({}), json!({ "title": "  " }), json!({ "title": 7 })] {
            let (resp, fx) = disp.dispatch("n", "notify", &args).await;
            assert!(!resp.ok, "{args}");
            assert!(fx.is_empty());
        }
        let long = "x".repeat(MAX_NOTIFY_TITLE_CHARS + 1);
        let (resp, fx) = disp
            .dispatch("n", "notify", &json!({ "title": long }))
            .await;
        assert!(resp.error.as_deref().unwrap().contains("exceeds"));
        assert!(fx.is_empty());

        let (resp, fx) = disp
            .dispatch(
                "n",
                "notify",
                &json!({ "title": " Tests  pass ", "body": "all\n2906 green" }),
            )
            .await;
        assert!(resp.ok);
        assert_eq!(resp.stdout.as_deref(), Some("notified"));
        let [RpcEvent::Named { name, payload }] = fx.as_slice() else {
            panic!("one event, got {fx:?}");
        };
        assert_eq!(name, EVENT_NOTIFY);
        assert_eq!(payload["title"], "Tests pass");
        assert_eq!(payload["body"], "all 2906 green");
    }
}
