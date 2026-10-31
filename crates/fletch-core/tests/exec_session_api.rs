use std::path::PathBuf;

use fletch_core::exec_session::{ExecCallbacks, ExecSession, ExecSpawn, TurnInvocation};
use fletch_core::sandbox::KillHandle;

fn spawn() -> ExecSpawn {
    ExecSpawn {
        program: PathBuf::from("unused"),
        prefix_args: Vec::new(),
        cwd: std::env::current_dir().unwrap(),
        session_id: None,
        stdout_is_json: true,
        env: Vec::new(),
        env_remove: Vec::new(),
        kill_plan: KillHandle::ProcessGroup,
        before_turn: None,
        after_exit: None,
    }
}

#[test]
fn public_constructor_keeps_accepting_argv_builders() {
    let _session = ExecSession::new(
        spawn(),
        |prompt, _, _, _| vec![prompt.to_string()],
        |_| None,
        ExecCallbacks {
            on_event: |_| {},
            on_session_id: |_| {},
            on_exit: |_| {},
        },
    );
}

#[test]
fn public_invocation_constructor_accepts_stdin() {
    let _session = ExecSession::new_with_invocation(
        spawn(),
        |prompt, _, _, _| TurnInvocation::with_stdin(Vec::new(), prompt.to_string()),
        |_| None,
        ExecCallbacks {
            on_event: |_| {},
            on_session_id: |_| {},
            on_exit: |_| {},
        },
    );
}
