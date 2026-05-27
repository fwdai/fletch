use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use notify_debouncer_mini::notify::{self, RecommendedWatcher};
use notify_debouncer_mini::{new_debouncer, Debouncer};

pub struct WatcherRegistry {
    watchers: HashMap<String, Debouncer<RecommendedWatcher>>,
}

impl WatcherRegistry {
    pub fn new() -> Self {
        Self {
            watchers: HashMap::new(),
        }
    }

    /// Register a debounced watcher.
    /// - `key`: namespaced string, e.g. "agent-abc123:git"
    /// - `paths`: list of paths to watch (files or directories)
    /// - `debounce`: how long to coalesce rapid events
    /// - `handler`: zero-arg closure called after debounce; callers re-query state themselves
    pub fn register(
        &mut self,
        key: &str,
        paths: Vec<PathBuf>,
        debounce: Duration,
        handler: impl Fn() + Send + 'static,
    ) -> Result<(), notify::Error> {
        // Drop any existing watcher under this key before creating the new one.
        self.watchers.remove(key);

        let mut debouncer = new_debouncer(debounce, move |result| {
            match result {
                Ok(_events) => handler(),
                Err(e) => {
                    tracing::warn!("watcher error: {:?}", e);
                }
            }
        })?;

        for path in paths {
            debouncer
                .watcher()
                .watch(&path, notify::RecursiveMode::Recursive)?;
        }

        self.watchers.insert(key.to_string(), debouncer);
        Ok(())
    }

    /// Remove a single watcher by exact key.
    pub fn unregister(&mut self, key: &str) {
        self.watchers.remove(key);
    }

    /// Remove all watchers whose key starts with `prefix` (e.g. pass agent_id to drop all of its watchers).
    pub fn unregister_prefix(&mut self, prefix: &str) {
        self.watchers.retain(|key, _| !key.starts_with(prefix));
    }
}

impl Default for WatcherRegistry {
    fn default() -> Self {
        Self::new()
    }
}
