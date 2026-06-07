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
    for (i, tok) in tokens.iter().enumerate() {
        // --port=N / -p=N / PORT=N
        if let Some(rest) = tok
            .strip_prefix("--port=")
            .or_else(|| tok.strip_prefix("-p="))
            .or_else(|| tok.strip_prefix("PORT="))
        {
            if let Ok(p) = rest.parse() {
                return Some(p);
            }
        }
        // --port N / -p N (value in the next token)
        if (*tok == "--port" || *tok == "-p") && i + 1 < tokens.len() {
            if let Ok(p) = tokens[i + 1].parse() {
                return Some(p);
            }
        }
        // bare :N (e.g. "serve :8080")
        if let Some(rest) = tok.strip_prefix(':') {
            if let Ok(p) = rest.parse() {
                return Some(p);
            }
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
}
