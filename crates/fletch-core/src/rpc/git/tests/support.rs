use std::path::Path;

use crate::rpc::caps::AgentCaps;
use crate::rpc::git::{GitDispatcher, EVENT_ACTION_DONE};
use crate::rpc::RpcEvent;

pub(super) fn write_request(requests: &Path, name: &str, body: &str) {
    std::fs::write(requests.join(name), body).unwrap();
}

pub(super) fn run_git(repo: &Path, args: &[&str]) {
    crate::git::testing::git(repo, args);
}

/// A dispatcher over `cwd`, with `cwd` standing in for the user's repo too:
/// a fixture is not an agent checkout, so there is no boundary to keep. Its
/// config does decide signing, which [`run_git`]'s isolation does not touch,
/// so the fixture is made [`unsigned`] here; the signing tests live in
/// `git::publish` and set their own policy.
pub(super) fn dispatcher(cwd: &Path) -> GitDispatcher {
    if cwd.join(".git").exists() {
        unsigned(cwd);
    }
    GitDispatcher::new(
        cwd.to_path_buf(),
        cwd.to_path_buf(),
        "main".to_string(),
        AgentCaps::interactive(),
    )
}

/// Make `repo` not sign, whatever the developer's global config says. The
/// publication layer reads the repo's effective `commit.gpgsign`, and a
/// global `true` with a key this process cannot reach would otherwise fail
/// every push a test makes.
pub(super) fn unsigned(repo: &Path) {
    run_git(repo, &["config", "commit.gpgsign", "false"]);
}

pub(super) fn has_action_done(effects: &[RpcEvent], expect_op: &str) -> bool {
    effects.iter().any(|e| {
        let RpcEvent::Named { name, payload } = e;
        name == EVENT_ACTION_DONE && payload["op"] == expect_op
    })
}
