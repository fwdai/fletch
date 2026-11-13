//! Every TCP port something listens on in a running box, forwarded to this
//! host: the same port when it is free here, else the next free one, as the
//! host's own Run bumps a taken port. `fletch-guest listeners` names the
//! ports listening in the guest, once and again on every change; each new one
//! gets a forward of its own (`Channel::forward`, an `ssh -L` connection), and
//! one that stops listening loses it. The watcher runs beside the box's
//! relays from its boot to its stop (`relay::Relays::forward_ports`), so it
//! never boots a box and never holds one against its idle stop. Whoever
//! follows the forwards hears every change ([`on_change`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::Arc;
use std::time::Duration;

use fletch_box::channel::Forward;
use fletch_box::{guest, Channel};
use parking_lot::RwLock;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::watch;

use crate::error::{Error, Result};

/// The most ports of one box forwarded at once: each is an ssh connection.
const MAX_FORWARDS: usize = 16;
/// How far past a guest port to look for a free port on this host.
const HOST_PORT_SCAN: u16 = 30;
/// How long a forward's host listener may take to come up: one ssh
/// connection through the proxy helper, handshake included.
const FORWARD_READY: Duration = Duration::from_secs(20);

/// A port something listens on in a box, and the port on this host that
/// reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ForwardedPort {
    pub guest_port: u16,
    pub port: u16,
}

type Listener = Arc<dyn Fn(&str, &[ForwardedPort]) + Send + Sync>;

static LISTENER: RwLock<Option<Listener>> = RwLock::new(None);

/// Have `listener` hear a box's agent id and its forwarded ports on every
/// change, and none once the box stops. Replaces the listener before it.
pub fn on_change(listener: impl Fn(&str, &[ForwardedPort]) + Send + Sync + 'static) {
    *LISTENER.write() = Some(Arc::new(listener));
}

fn tell(agent_id: &str, ports: &[ForwardedPort]) {
    let listener = LISTENER.read().clone();
    if let Some(listener) = listener {
        listener(agent_id, ports);
    }
}

/// Follow `fletch-guest listeners` in `channel`'s guest until `stop` says so,
/// reconnecting as a relay does, with a forward for each port it names.
pub(super) fn spawn(
    channel: Arc<Channel>,
    agent_id: &str,
    stop: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let agent_id = agent_id.to_string();
    crate::host::spawn(async move {
        super::relay::run_with_reconnect(
            stop,
            || channel.spawn_relay(&[guest::LISTENERS]),
            |child, stop| pump_listeners(child, channel.clone(), agent_id.clone(), stop),
        )
        .await;
    })
}

/// Forward what `child`, one `listeners` session, names until it ends or
/// `stop` says so. Its forwards end with it ([`Forwards`]).
async fn pump_listeners(
    mut child: Child,
    channel: Arc<Channel>,
    agent_id: String,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("listeners has no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut forwards = Forwards {
        agent_id,
        open: BTreeMap::new(),
    };
    loop {
        tokio::select! {
            line = lines.next_line() => match line? {
                Some(line) => match parse_listeners(&line) {
                    Ok(listening) => forwards.follow(&channel, &listening).await,
                    Err(e) => tracing::warn!(error = %e, %line, "box ports: bad listeners line"),
                },
                None => return Ok(()),
            },
            _ = stop.changed() => {
                let _ = child.start_kill();
                return Ok(());
            }
        }
    }
}

/// One line of `fletch-guest listeners`: the guest ports listening now.
fn parse_listeners(line: &str) -> serde_json::Result<BTreeSet<u16>> {
    serde_json::from_str(line)
}

/// What makes the forwards of a box follow what its guest listens on.
#[derive(Debug, Default, PartialEq, Eq)]
struct Change {
    /// Guest ports to forward, lowest first.
    open: Vec<u16>,
    /// Guest ports whose forward goes.
    close: Vec<u16>,
}

/// The change that makes `forwarded`, the guest ports with a forward, follow
/// `listening`: a port no longer listening loses its forward, and a new one
/// gets one while fewer than `cap` are forwarded, lowest first. A forward
/// that stays is never given up for a lower port.
fn reconcile(forwarded: &BTreeSet<u16>, listening: &BTreeSet<u16>, cap: usize) -> Change {
    let close: Vec<u16> = forwarded.difference(listening).copied().collect();
    let kept = forwarded.len() - close.len();
    let open = listening
        .difference(forwarded)
        .copied()
        .take(cap.saturating_sub(kept))
        .collect();
    Change { open, close }
}

/// The forwards of one `listeners` session, by guest port. They end when it
/// is dropped, however its session ended, and the listener hears so.
struct Forwards {
    agent_id: String,
    open: BTreeMap<u16, Forward>,
}

impl Forwards {
    fn ports(&self) -> Vec<ForwardedPort> {
        self.open
            .iter()
            .map(|(&guest_port, forward)| ForwardedPort {
                guest_port,
                port: forward.host_port,
            })
            .collect()
    }

    /// Open and close forwards so they follow `listening`, then tell the
    /// listener what is forwarded. A port that cannot be forwarded now is
    /// tried again with the next change.
    async fn follow(&mut self, channel: &Channel, listening: &BTreeSet<u16>) {
        let forwarded = self.open.keys().copied().collect();
        let change = reconcile(&forwarded, listening, MAX_FORWARDS);
        if change == Change::default() {
            return;
        }
        for guest_port in change.close {
            self.open.remove(&guest_port);
        }
        for guest_port in change.open {
            match forward(channel, guest_port).await {
                Ok(forward) => {
                    self.open.insert(guest_port, forward);
                }
                Err(e) => tracing::warn!(
                    agent_id = %self.agent_id,
                    guest_port,
                    error = %e,
                    "could not forward a port of the box"
                ),
            }
        }
        tell(&self.agent_id, &self.ports());
    }
}

impl Drop for Forwards {
    fn drop(&mut self) {
        if !self.open.is_empty() {
            self.open.clear();
            tell(&self.agent_id, &[]);
        }
    }
}

/// `guest_port` forwarded to a free port on this host, its listener up.
async fn forward(channel: &Channel, guest_port: u16) -> Result<Forward> {
    let port =
        crate::run_detect::port::find_free_port(guest_port, HOST_PORT_SCAN).ok_or_else(|| {
            Error::Other(format!(
                "no free port on this host in {guest_port}\u{2013}{}",
                guest_port.saturating_add(HOST_PORT_SCAN)
            ))
        })?;
    let mut forward = channel
        .forward(port, guest_port)
        .map_err(|e| Error::Other(e.to_string()))?;
    forward
        .wait_ready(FORWARD_READY)
        .await
        .map_err(|e| Error::Other(e.to_string()))?;
    Ok(forward)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ports: &[u16]) -> BTreeSet<u16> {
        ports.iter().copied().collect()
    }

    #[test]
    fn a_new_port_is_opened_and_a_gone_one_closed() {
        assert_eq!(
            reconcile(&set(&[3000, 5432]), &set(&[3000, 8080]), MAX_FORWARDS),
            Change {
                open: vec![8080],
                close: vec![5432]
            }
        );
        assert_eq!(
            reconcile(&set(&[]), &set(&[5432, 3000]), MAX_FORWARDS),
            Change {
                open: vec![3000, 5432],
                close: vec![]
            }
        );
        assert_eq!(
            reconcile(&set(&[3000]), &set(&[]), MAX_FORWARDS),
            Change {
                open: vec![],
                close: vec![3000]
            }
        );
        assert_eq!(
            reconcile(&set(&[3000]), &set(&[3000]), MAX_FORWARDS),
            Change::default()
        );
    }

    /// Past the cap the lowest new ports get the room left, and a forward
    /// already up keeps its place even when a lower port comes along.
    #[test]
    fn the_cap_keeps_what_is_forwarded_and_fills_up_lowest_first() {
        assert_eq!(
            reconcile(&set(&[]), &set(&[9000, 1000, 5000]), 2),
            Change {
                open: vec![1000, 5000],
                close: vec![]
            }
        );
        assert_eq!(
            reconcile(&set(&[5000, 9000]), &set(&[1000, 5000, 9000]), 2),
            Change::default()
        );
        // One goes: the room it leaves takes the lowest waiting port.
        assert_eq!(
            reconcile(&set(&[5000, 9000]), &set(&[1000, 2000, 5000]), 2),
            Change {
                open: vec![1000],
                close: vec![9000]
            }
        );
    }

    #[test]
    fn a_listeners_line_is_a_json_array_of_ports() {
        assert_eq!(parse_listeners("[3000,5432]").unwrap(), set(&[3000, 5432]));
        assert_eq!(parse_listeners("[]").unwrap(), set(&[]));
        assert_eq!(parse_listeners(" [80] ").unwrap(), set(&[80]));
        for bad in [
            "",
            "3000",
            "[70000]",
            "[-1]",
            "{\"ports\":[]}",
            "[\"3000\"]",
        ] {
            assert!(parse_listeners(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_forwarded_port_serializes_as_the_clients_read_it() {
        assert_eq!(
            serde_json::to_value(ForwardedPort {
                guest_port: 3000,
                port: 3001
            })
            .unwrap(),
            serde_json::json!({ "guest_port": 3000, "port": 3001 })
        );
    }
}
