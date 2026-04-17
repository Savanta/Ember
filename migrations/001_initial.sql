CREATE TABLE IF NOT EXISTS notifications (
    id          INTEGER PRIMARY KEY,
    app_name    TEXT    NOT NULL,
    summary     TEXT    NOT NULL,
    body        TEXT    NOT NULL DEFAULT '',
    icon        TEXT    NOT NULL DEFAULT '',
    urgency     INTEGER NOT NULL DEFAULT 1,
    timestamp   INTEGER NOT NULL,
    source_id   INTEGER NOT NULL DEFAULT 0,
    actions     TEXT    NOT NULL DEFAULT '[]',
    hints       TEXT    NOT NULL DEFAULT '{}',
    expire_timeout INTEGER NOT NULL DEFAULT -1,
    state       INTEGER NOT NULL DEFAULT 0,
    group_key   TEXT,
    can_reply   INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_notifications_timestamp ON notifications(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_notifications_state     ON notifications(state);
CREATE INDEX IF NOT EXISTS idx_notifications_app_name  ON notifications(app_name);
