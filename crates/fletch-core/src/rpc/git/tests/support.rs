use std::path::Path;

use crate::rpc::caps::AgentCaps;
use crate::rpc::git::{GitDispatcher, EVENT_ACTION_DONE};
use crate::rpc::RpcEvent;

pub(super) fn write_request(requests: &Path, name: &str, body: &str) {
    std::fs::write(requests.join(name), body).unwrap();
}

pub(super) fn run_git(repo: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub(super) fn dispatcher(cwd: &Path) -> GitDispatcher {
    GitDispatcher::new(
        cwd.to_path_buf(),
        "main".to_string(),
        AgentCaps::interactive(),
    )
}

pub(super) fn has_action_done(effects: &[RpcEvent], expect_op: &str) -> bool {
    effects.iter().any(|e| {
        let RpcEvent::Named { name, payload } = e;
        name == EVENT_ACTION_DONE && payload["op"] == expect_op
    })
}
