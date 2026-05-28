CREATE TABLE project_settings (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (project_id, key)
);

CREATE INDEX idx_project_settings_project_id ON project_settings(project_id);
