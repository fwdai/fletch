//! A provider account in memory behind a stand-in server ([`super::stand_in`]):
//! what a test outside `cloud/` drives a box's machine with, naming no
//! provider. It answers the API of the provider its [`client`](FakeCloud::client)
//! is for as that provider would, holding each box as a disk and a machine on
//! it, either of which a test can take away ([`FakeCloud::shape`]), and keeps
//! every act on a box under a neutral name ([`FakeCloud::acts`]).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::stand_in::{Reply, Request, StandIn};
use super::{fly, Client, Connection, ProviderId};

/// The host key every machine's guest has.
pub(crate) const HOST_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 fletch-box";

const ORG: &str = "acme";

/// What a box is left as, for a test to put every box into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Running,
    Stopped,
    /// The machine deleted, the disk kept.
    MachineGone,
    /// The disk deleted.
    DiskGone,
    /// Nothing of the box left.
    Gone,
}

/// One box: its disk and its machine.
struct Box {
    disk: Option<String>,
    machine: Option<(String, &'static str)>,
    /// A machine was made for it: the next one replaces it.
    had_machine: bool,
}

#[derive(Default)]
struct Account {
    boxes: BTreeMap<String, Box>,
    acts: Vec<&'static str>,
    refuse_destroy: bool,
    next_id: u32,
}

pub(crate) struct FakeCloud {
    server: StandIn,
    account: Arc<Mutex<Account>>,
}

impl FakeCloud {
    pub(crate) async fn start() -> Self {
        let account = Arc::new(Mutex::new(Account::default()));
        let held = account.clone();
        let server = StandIn::start(move |r| answer(&mut held.lock().unwrap(), r)).await;
        Self { server, account }
    }

    /// A client for this account, pacing its waits in milliseconds.
    pub(crate) fn client(&self) -> Client {
        let connection =
            Connection::new(ProviderId::Fly, "fake-token-0123456789", Some(ORG)).expect("a token");
        Client::Fly(
            fly::Client::new(&connection, self.server.url())
                .expect("a client")
                .with_pace(Duration::from_millis(1)),
        )
    }

    /// Every act on a box so far, in order: `create`, `start`, `stop`,
    /// `park` (the machine put at rest the provider's fastest way back),
    /// `replace`, `exec`, `destroy`.
    pub(crate) fn acts(&self) -> Vec<&'static str> {
        self.account.lock().unwrap().acts.clone()
    }

    /// How many boxes the account holds, whatever is left of each.
    pub(crate) fn boxes(&self) -> usize {
        self.account.lock().unwrap().boxes.len()
    }

    /// Put every box into `shape`.
    pub(crate) fn shape(&self, shape: Shape) {
        let mut account = self.account.lock().unwrap();
        if shape == Shape::Gone {
            account.boxes.clear();
            return;
        }
        for b in account.boxes.values_mut() {
            match shape {
                Shape::Running | Shape::Stopped => {
                    let state = if shape == Shape::Running {
                        "started"
                    } else {
                        "stopped"
                    };
                    if let Some((_, s)) = &mut b.machine {
                        *s = state;
                    }
                }
                Shape::MachineGone => b.machine = None,
                Shape::DiskGone => b.disk = None,
                Shape::Gone => unreachable!("handled above"),
            }
        }
    }

    /// Refuse every delete from now on, as an unreachable provider would.
    pub(crate) fn refuse_destroy(&self) {
        self.account.lock().unwrap().refuse_destroy = true;
    }
}

/// The provider's answer to `r`, from `account`.
fn answer(account: &mut Account, r: &Request) -> Reply {
    let path = r.path.split('?').next().unwrap_or_default();
    let query = r.path.split_once('?').map_or("", |(_, q)| q);
    let parts: Vec<&str> = path.trim_start_matches("/v1/apps").split('/').collect();
    let not_found = || Reply::json(404, json!({ "error": "not found" }));
    let method = r.method.as_str();
    if method == "DELETE" && account.refuse_destroy {
        return Reply::json(503, json!({ "error": "unavailable" }));
    }
    match (method, parts.as_slice()) {
        ("GET", [""]) => Reply::json(
            200,
            json!({ "apps": account.boxes.keys().map(|name| json!({ "name": name })).collect::<Vec<_>>() }),
        ),
        ("POST", [""]) => {
            let name = r.body["name"].as_str().unwrap_or_default().to_string();
            account.boxes.insert(
                name.clone(),
                Box {
                    disk: None,
                    machine: None,
                    had_machine: false,
                },
            );
            account.acts.push("create");
            Reply::json(201, json!({ "id": name }))
        }
        ("DELETE", ["", app]) => {
            if account.boxes.remove(*app).is_none() {
                return not_found();
            }
            account.acts.push("destroy");
            Reply::json(202, json!({}))
        }
        (_, ["", app, rest @ ..]) => {
            let next_id = account.next_id;
            account.next_id += 1;
            let Account { boxes, acts, .. } = account;
            let Some(b) = boxes.get_mut(*app) else {
                return not_found();
            };
            match (method, rest) {
                ("GET", []) => Reply::json(200, json!({ "name": app })),
                ("POST", ["ip_assignments"]) => Reply::json(200, json!({})),
                ("POST", ["volumes"]) => {
                    let id = format!("vol_{next_id}");
                    b.disk = Some(id.clone());
                    Reply::json(
                        200,
                        json!({ "id": id, "name": "fletch_data", "state": "created", "region": "ams" }),
                    )
                }
                ("GET", ["volumes"]) => Reply::json(
                    200,
                    json!(b
                        .disk
                        .iter()
                        .map(|id| json!({ "id": id, "name": "fletch_data", "state": "created" }))
                        .collect::<Vec<_>>()),
                ),
                ("GET", ["volumes", id]) if b.disk.as_deref() == Some(*id) => Reply::json(
                    200,
                    json!({ "id": id, "state": "created", "region": "ams" }),
                ),
                ("DELETE", ["volumes", id]) if b.disk.as_deref() == Some(*id) => {
                    b.disk = None;
                    Reply::json(200, json!({}))
                }
                ("POST", ["machines"]) => {
                    let id = format!("m{next_id}");
                    if b.had_machine {
                        acts.push("replace");
                    }
                    b.had_machine = true;
                    b.machine = Some((id.clone(), "started"));
                    Reply::json(200, json!({ "id": id, "state": "created" }))
                }
                ("GET", ["machines"]) => Reply::json(
                    200,
                    json!(b
                        .machine
                        .iter()
                        .map(|(id, state)| json!({
                            "id": id,
                            "state": state,
                            "config": { "metadata": { "fletch_box": app } },
                        }))
                        .collect::<Vec<_>>()),
                ),
                (_, ["machines", id, action @ ..]) => {
                    let Some((_, state)) = b.machine.as_mut().filter(|(m, _)| m == id) else {
                        return not_found();
                    };
                    match (method, action) {
                        ("GET", []) => {
                            Reply::json(200, json!({ "id": id, "state": state, "version": "v1" }))
                        }
                        ("GET", ["wait"]) => {
                            let wanted = query
                                .split('&')
                                .find_map(|kv| kv.strip_prefix("state="))
                                .unwrap_or_default();
                            if wanted == *state {
                                Reply::json(200, json!({ "ok": true }))
                            } else {
                                Reply::json(408, json!({ "error": "deadline_exceeded" }))
                            }
                        }
                        ("POST", ["start"]) => {
                            *state = "started";
                            acts.push("start");
                            Reply::json(200, json!({}))
                        }
                        ("POST", ["stop"]) => {
                            *state = "stopped";
                            acts.push("stop");
                            Reply::json(200, json!({}))
                        }
                        ("POST", ["suspend"]) => {
                            *state = "suspended";
                            acts.push("park");
                            Reply::json(200, json!({ "ok": true }))
                        }
                        ("POST", ["exec"]) if *state == "started" => {
                            acts.push("exec");
                            Reply::json(
                                200,
                                json!({ "exit_code": 0, "stdout": format!("{HOST_KEY}\n"), "stderr": "" }),
                            )
                        }
                        ("POST", ["exec"]) => {
                            Reply::json(412, json!({ "error": "machine not running" }))
                        }
                        ("DELETE", []) => {
                            b.machine = None;
                            Reply::json(200, json!({}))
                        }
                        _ => {
                            Reply::json(500, json!({ "error": format!("unexpected {}", r.line()) }))
                        }
                    }
                }
                _ => not_found(),
            }
        }
        _ => Reply::json(500, json!({ "error": format!("unexpected {}", r.line()) })),
    }
}
