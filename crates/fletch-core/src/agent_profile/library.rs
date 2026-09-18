//! Resolving a stored custom-agent preset into the by-value profile a spawn
//! carries: its standing brief plus snapshots of the skills and MCP servers it
//! has assigned.
//!
//! The desktop does this in the webview (`src/helpers/spawn.ts`), out of the
//! library state it already holds. A phone holds none of it — and must not:
//! MCP rows carry commands, tokens and headers that have no business crossing
//! the wire. So a remote spawn names the preset and the host resolves it here,
//! against the same tables, by the same rules.

use rusqlite::Connection;
use serde_json::{Map, Value};

use super::{McpServerSnapshot, SkillSnapshot};
use crate::database::db_select;

/// Everything a custom agent contributes to a spawn, resolved by value —
/// snapshotted rather than referenced, so a later library edit never reaches a
/// running session.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomAgentProfile {
    pub custom_agent_id: String,
    pub instructions: Option<String>,
    pub skills: Vec<SkillSnapshot>,
    pub mcp_servers: Vec<McpServerSnapshot>,
}

/// None when the id does not resolve to a row (a dangling id must not be
/// stamped on the session).
///
/// Assignments keep the agent's own order; ids that no longer resolve drop out,
/// as do MCP servers `provider` cannot deliver — the snapshot must hold exactly
/// what the agent will get, never an assignment silently ignored downstream.
pub fn resolve_custom_agent(
    conn: &Connection,
    custom_agent_id: &str,
    provider: &str,
) -> Option<CustomAgentProfile> {
    let agent = row_by_id(conn, "custom_agents", custom_agent_id)?;

    let skills = id_list(&agent, "skill_ids")
        .iter()
        .filter_map(|id| row_by_id(conn, "skills", id))
        .map(|r| SkillSnapshot {
            name: text(&r, "name"),
            description: text(&r, "description"),
            body: text(&r, "body"),
        })
        .collect();

    let mcp_servers = id_list(&agent, "mcp_server_ids")
        .iter()
        .filter_map(|id| row_by_id(conn, "mcp_servers", id))
        .filter(|r| attachable(provider, &text(r, "transport")))
        .map(|r| snapshot_server(&r))
        .collect();

    let instructions = text(&agent, "instructions").trim().to_string();
    Some(CustomAgentProfile {
        custom_agent_id: text(&agent, "id"),
        instructions: (!instructions.is_empty()).then_some(instructions),
        skills,
        mcp_servers,
    })
}

/// Whether `provider` can actually deliver a server of `transport`, mirroring
/// the editor's `mcpAttachable`. Whether it speaks MCP at all comes from the
/// capability table; codex is the one provider whose delivery is stdio-only
/// (`codex_mcp_args` drops the rest), so an http row would be snapshotted and
/// then quietly ignored.
fn attachable(provider: &str, transport: &str) -> bool {
    crate::agent::mcp_delivery(provider).is_some() && !(provider == "codex" && transport == "http")
}

/// A registry row as the spawn payload wants it: the command line is
/// whitespace-split into command + args (no shell quoting — args with spaces
/// are documented as unsupported), env/header lines parsed into pairs.
fn snapshot_server(server: &Map<String, Value>) -> McpServerSnapshot {
    let command = text(server, "command");
    let mut tokens = command.split_whitespace().map(str::to_string);
    McpServerSnapshot {
        name: text(server, "name"),
        transport: text(server, "transport"),
        command: tokens.next().unwrap_or_default(),
        args: tokens.collect(),
        env: parse_pairs(&text(server, "env"), '='),
        url: text(server, "url").trim().to_string(),
        headers: parse_pairs(&text(server, "headers"), ':'),
    }
}

/// One pair per line, split on the first `sep` (so a value may contain it).
/// Blanks, lines without a separator and lines with an empty key are skipped.
/// Both stored forms — `KEY=VALUE` env and `Name: value` headers — differ only
/// in that character.
fn parse_pairs(text: &str, sep: char) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| line.trim().split_once(sep))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

/// One row by primary key, or `None` when the id is dangling.
fn row_by_id(conn: &Connection, table: &str, id: &str) -> Option<Map<String, Value>> {
    db_select(conn, table, serde_json::json!({ "where": { "id": id } }))
        .ok()?
        .into_iter()
        .next()
}

/// A TEXT column as a string; absent or non-text reads as empty, matching the
/// schema's `NOT NULL DEFAULT ''`.
fn text(row: &Map<String, Value>, column: &str) -> String {
    row.get(column)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `*_ids` column: a JSON text array, empty when it is anything else.
fn id_list(row: &Map<String, Value>, column: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&text(row, column)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::database::get_migrations()
            .to_latest(&mut conn)
            .unwrap();
        conn
    }

    fn skill(conn: &Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO skills (id, name, description, body, created_at, updated_at)
             VALUES (?1, ?2, 'when to use it', '# body', 0, 0)",
            rusqlite::params![id, name],
        )
        .unwrap();
    }

    fn server(conn: &Connection, id: &str, transport: &str) {
        conn.execute(
            "INSERT INTO mcp_servers (id, name, transport, command, env, url, headers,
                                      created_at, updated_at)
             VALUES (?1, ?1, ?2, 'npx -y  gh-mcp', 'TOKEN=a=b\n\nbad\n=nokey\n',
                     ' https://mcp.example ', 'Authorization: Bearer t\nno-colon\n', 0, 0)",
            rusqlite::params![id, transport],
        )
        .unwrap();
    }

    fn custom_agent(conn: &Connection, id: &str, instructions: &str, skills: &str, servers: &str) {
        conn.execute(
            "INSERT INTO custom_agents (id, name, description, color, base, instructions,
                                        skill_ids, mcp_server_ids, created_at, updated_at)
             VALUES (?1, 'PM', '', 200, 'claude', ?2, ?3, ?4, 0, 0)",
            rusqlite::params![id, instructions, skills, servers],
        )
        .unwrap();
    }

    #[test]
    fn env_and_header_lines_split_on_the_first_separator() {
        assert_eq!(
            parse_pairs("TOKEN=a=b\n\n  KEY = v  \nbad\n=nokey\n", '='),
            vec![
                ("TOKEN".to_string(), "a=b".to_string()),
                ("KEY".to_string(), "v".to_string()),
            ]
        );
        assert_eq!(
            parse_pairs("Authorization: Bearer t\nX-Trace:1\nno-colon", ':'),
            vec![
                ("Authorization".to_string(), "Bearer t".to_string()),
                ("X-Trace".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn a_command_line_splits_into_command_and_args() {
        let conn = test_conn();
        server(&conn, "m1", "stdio");
        let snap = snapshot_server(&row_by_id(&conn, "mcp_servers", "m1").unwrap());
        assert_eq!(snap.command, "npx");
        assert_eq!(snap.args, vec!["-y".to_string(), "gh-mcp".to_string()]);
        assert_eq!(snap.env, vec![("TOKEN".to_string(), "a=b".to_string())]);
        assert_eq!(snap.url, "https://mcp.example");
        assert_eq!(
            snap.headers,
            vec![("Authorization".to_string(), "Bearer t".to_string())]
        );
    }

    #[test]
    fn assignments_resolve_in_order_and_dangling_ids_drop_out() {
        let conn = test_conn();
        skill(&conn, "s1", "review");
        skill(&conn, "s2", "ship");
        server(&conn, "m1", "stdio");
        custom_agent(
            &conn,
            "ca-1",
            "  You are the Project Manager.  ",
            r#"["s2","gone","s1"]"#,
            r#"["gone","m1"]"#,
        );

        let profile = resolve_custom_agent(&conn, "ca-1", "claude").unwrap();
        assert_eq!(profile.custom_agent_id, "ca-1");
        assert_eq!(
            profile.instructions.as_deref(),
            Some("You are the Project Manager.")
        );
        assert_eq!(
            profile.skills.iter().map(|s| &s.name).collect::<Vec<_>>(),
            vec!["ship", "review"]
        );
        assert_eq!(profile.mcp_servers.len(), 1);
        assert_eq!(profile.mcp_servers[0].name, "m1");
    }

    #[test]
    fn a_blank_brief_injects_nothing_and_an_unknown_id_resolves_to_nothing() {
        let conn = test_conn();
        custom_agent(&conn, "ca-1", "   ", "[]", "[]");

        let profile = resolve_custom_agent(&conn, "ca-1", "claude").unwrap();
        assert!(profile.instructions.is_none());
        assert!(profile.skills.is_empty() && profile.mcp_servers.is_empty());

        assert!(resolve_custom_agent(&conn, "ca-nope", "claude").is_none());
    }

    /// The provider filter is the editor's: claude/opencode take both
    /// transports, codex only stdio, and a provider with no MCP surface gets
    /// none of them.
    #[test]
    fn servers_the_provider_cannot_deliver_are_left_out() {
        let conn = test_conn();
        server(&conn, "stdio-1", "stdio");
        server(&conn, "http-1", "http");
        custom_agent(&conn, "ca-1", "", "[]", r#"["stdio-1","http-1"]"#);

        let names = |provider: &str| {
            resolve_custom_agent(&conn, "ca-1", provider)
                .unwrap()
                .mcp_servers
                .into_iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("claude"), vec!["stdio-1", "http-1"]);
        assert_eq!(names("opencode"), vec!["stdio-1", "http-1"]);
        assert_eq!(names("codex"), vec!["stdio-1"]);
        assert!(names("pi").is_empty());
    }
}
