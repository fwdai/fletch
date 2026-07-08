//! Port detection: scan the resolved dev/run command for an explicit
//! port token, else fall back to a framework-default table. No config
//! file parsing.

/// Detect a port for `dev_cmd`, given the set of dependency names present
/// in the project (lowercased). Returns `(port, source)` or None when no
/// port can be inferred (e.g. Rust/Go/plain scripts).
pub fn detect_port(dev_cmd: &str, deps: &[String]) -> Option<(u16, String)> {
    if let Some(p) = explicit_port(dev_cmd) {
        return Some((p, "dev command".to_string()));
    }
    framework_default(deps).map(|(p, fw)| (p, format!("default ({fw})")))
}

/// Scan a command string for an explicit port token in any of the common
/// forms: `--port 3001`, `--port=3001`, `-p 3001`, `PORT=3001`, `:3001`.
fn explicit_port(cmd: &str) -> Option<u16> {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    find_port_token(&tokens).map(|(_, port)| port)
}

/// Locate the first explicit port token in `tokens`, returning the index of
/// the token that carries the numeric value and the parsed port. For the
/// space-separated forms (`--port N`, `-p N`) that's the *value* token
/// (`i + 1`), not the flag; for the fused forms (`--port=N`, `:N`) it's the
/// token itself. Shared by [`explicit_port`] and [`rewrite_explicit_port`] so
/// the recognized forms never drift.
fn find_port_token(tokens: &[&str]) -> Option<(usize, u16)> {
    for (i, tok) in tokens.iter().enumerate() {
        // --port=N / -p=N / PORT=N
        if let Some(rest) = tok
            .strip_prefix("--port=")
            .or_else(|| tok.strip_prefix("-p="))
            .or_else(|| tok.strip_prefix("PORT="))
        {
            if let Ok(p) = rest.parse() {
                return Some((i, p));
            }
        }
        // --port N / -p N (value in the next token)
        if (*tok == "--port" || *tok == "-p") && i + 1 < tokens.len() {
            if let Ok(p) = tokens[i + 1].parse() {
                return Some((i + 1, p));
            }
        }
        // bare :N (e.g. "serve :8080")
        if let Some(rest) = tok.strip_prefix(':') {
            if let Ok(p) = rest.parse() {
                return Some((i, p));
            }
        }
    }
    None
}

/// Rewrite the first explicit port token in `cmd` to `new_port`, preserving the
/// token's form (`--port=N`, `-p N`, `:N`, `PORT=N`, …). Returns `None` when
/// `cmd` has no explicit port token — the caller then relies on the `PORT` env
/// var alone. Tokens are rejoined on single spaces, which is fine for the
/// simple dev commands detection targets.
pub fn rewrite_explicit_port(cmd: &str, new_port: u16) -> Option<String> {
    let mut tokens: Vec<String> = cmd.split_whitespace().map(|s| s.to_string()).collect();
    let refs: Vec<&str> = tokens.iter().map(|s| s.as_str()).collect();
    let (idx, _) = find_port_token(&refs)?;
    let tok = &tokens[idx];
    // Rebuild the value token in its original form. The value token is either a
    // bare number (space-separated form) or `<prefix>=<n>` / `:<n>`.
    let rewritten = if let Some(prefix) = tok
        .strip_prefix("--port=")
        .map(|_| "--port=")
        .or_else(|| tok.strip_prefix("-p=").map(|_| "-p="))
        .or_else(|| tok.strip_prefix("PORT=").map(|_| "PORT="))
    {
        format!("{prefix}{new_port}")
    } else if tok.starts_with(':') {
        format!(":{new_port}")
    } else {
        // Space-separated form: the token is the bare number.
        new_port.to_string()
    };
    tokens[idx] = rewritten;
    Some(tokens.join(" "))
}

/// Find the first free TCP port at or after `start`, scanning `start`,
/// `start + 1`, … up to and including `start + cap`. "Free" means we can bind
/// `127.0.0.1:<port>` right now (the listener is dropped immediately, releasing
/// it for the dev server to claim). Returns `None` if every port in the range
/// is taken. Binding-to-test is the standard technique; there is no lighter
/// primitive that also honors the "your port, then +1, +2, …" order.
pub fn find_free_port(start: u16, cap: u16) -> Option<u16> {
    for offset in 0..=cap {
        let port = start.checked_add(offset)?;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

/// Map a known framework dependency to its conventional dev-server port.
/// First match wins; order matters where defaults overlap.
///
/// `detect_port` is currently called only by the Node detector, whose
/// `deps` are npm package names — so this table lists only JS frameworks.
/// Python/Ruby frameworks (Django, Flask, Rails) set their ports directly
/// in their own detectors; adding them here would be unreachable.
fn framework_default(deps: &[String]) -> Option<(u16, &'static str)> {
    const TABLE: &[(&str, u16, &str)] = &[
        ("next", 3000, "Next.js"),
        ("nuxt", 3000, "Nuxt"),
        ("@remix-run/dev", 3000, "Remix"),
        ("astro", 4321, "Astro"),
        ("vite", 5173, "Vite"),
        ("react-scripts", 3000, "CRA"),
        ("@angular/core", 4200, "Angular"),
        ("vue-cli-service", 8080, "Vue CLI"),
    ];
    for (name, port, label) in TABLE {
        if deps.iter().any(|d| d == name) {
            return Some((*port, label));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deps(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn explicit_flag_space_separated() {
        assert_eq!(detect_port("vite --port 4000", &[]).unwrap().0, 4000);
    }

    #[test]
    fn explicit_flag_equals() {
        assert_eq!(detect_port("vite --port=4001", &[]).unwrap().0, 4001);
    }

    #[test]
    fn explicit_short_flag() {
        assert_eq!(detect_port("next dev -p 4002", &[]).unwrap().0, 4002);
    }

    #[test]
    fn explicit_env_assignment() {
        assert_eq!(detect_port("PORT=4003 node server.js", &[]).unwrap().0, 4003);
    }

    #[test]
    fn explicit_colon_form() {
        assert_eq!(detect_port("serve :8081", &[]).unwrap().0, 8081);
    }

    #[test]
    fn explicit_beats_framework_default() {
        let (port, source) = detect_port("vite --port 9000", &deps(&["vite"])).unwrap();
        assert_eq!(port, 9000);
        assert_eq!(source, "dev command");
    }

    #[test]
    fn framework_default_when_no_flag() {
        let (port, source) = detect_port("vite", &deps(&["vite"])).unwrap();
        assert_eq!(port, 5173);
        assert_eq!(source, "default (Vite)");
    }

    #[test]
    fn next_default_is_3000() {
        assert_eq!(detect_port("next dev", &deps(&["next"])).unwrap().0, 3000);
    }

    #[test]
    fn unknown_framework_no_flag_is_none() {
        assert!(detect_port("node server.js", &deps(&["express"])).is_none());
    }

    #[test]
    fn empty_is_none() {
        assert!(detect_port("", &[]).is_none());
    }

    // ── rewrite_explicit_port ──────────────────────────────────────────────

    #[test]
    fn rewrite_space_separated() {
        assert_eq!(
            rewrite_explicit_port("vite --port 3000", 3001).unwrap(),
            "vite --port 3001"
        );
    }

    #[test]
    fn rewrite_short_flag() {
        assert_eq!(
            rewrite_explicit_port("next dev -p 3000", 3005).unwrap(),
            "next dev -p 3005"
        );
    }

    #[test]
    fn rewrite_equals_form() {
        assert_eq!(
            rewrite_explicit_port("vite --port=3000", 3002).unwrap(),
            "vite --port=3002"
        );
    }

    #[test]
    fn rewrite_env_assignment() {
        assert_eq!(
            rewrite_explicit_port("PORT=3000 node server.js", 3003).unwrap(),
            "PORT=3003 node server.js"
        );
    }

    #[test]
    fn rewrite_colon_form() {
        assert_eq!(
            rewrite_explicit_port("serve :8080", 8081).unwrap(),
            "serve :8081"
        );
    }

    #[test]
    fn rewrite_none_when_no_token() {
        assert!(rewrite_explicit_port("pnpm dev", 3001).is_none());
    }

    // ── find_free_port ─────────────────────────────────────────────────────

    #[test]
    fn free_port_returns_start_when_open() {
        // Bind an ephemeral port to learn a definitely-free number, release it,
        // then confirm the finder returns it as the start.
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        assert_eq!(find_free_port(port, 30), Some(port));
    }

    #[test]
    fn free_port_skips_occupied() {
        // Hold a listener open on `port`, so the finder must skip to `port + 1`.
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        // Guard against the (rare) chance port+1 is also busy on the test host.
        assert!(port < u16::MAX);
        let found = find_free_port(port, 30).unwrap();
        assert_ne!(found, port);
        assert!(found > port);
    }
}
