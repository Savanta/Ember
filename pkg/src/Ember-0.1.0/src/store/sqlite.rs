use std::path::Path;
use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Row, SqlitePool};

use crate::store::models::{
    Notification, NotificationAction, NotificationState, Urgency,
};

#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Open (or create) the database at `path` and run migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db dir {}", parent.display()))?;
        }

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);

        let pool = SqlitePool::connect_with(opts)
            .await
            .with_context(|| format!("open sqlite at {}", path.display()))?;

        sqlx::raw_sql(include_str!("../../migrations/001_initial.sql"))
            .execute(&pool)
            .await
            .context("run migrations")?;

        Ok(Self { pool })
    }

    /// Persist a notification (insert or replace).
    pub async fn upsert(&self, notif: &Notification) -> Result<()> {
        let actions = serde_json::to_string(&notif.actions)?;
        let hints   = serde_json::to_string(&notif.hints)?;

        sqlx::query(
            "INSERT OR REPLACE INTO notifications
             (id, app_name, summary, body, icon, urgency, timestamp, source_id,
              actions, hints, expire_timeout, state, group_key, can_reply)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(notif.id as i64)
        .bind(&notif.app_name)
        .bind(&notif.summary)
        .bind(&notif.body)
        .bind(&notif.icon)
        .bind(notif.urgency as i64)
        .bind(notif.timestamp)
        .bind(notif.source_id as i64)
        .bind(&actions)
        .bind(&hints)
        .bind(notif.expire_timeout)
        .bind(notif.state as i64)
        .bind(&notif.group_key)
        .bind(notif.can_reply as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Update only the state field.
    pub async fn update_state(&self, id: u32, state: NotificationState) -> Result<()> {
        sqlx::query("UPDATE notifications SET state = ? WHERE id = ?")
            .bind(state as i64)
            .bind(id as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Load paginated history ordered by timestamp DESC.
    pub async fn history(&self, limit: usize, offset: usize) -> Result<Vec<Notification>> {
        let rows = sqlx::query(
            "SELECT id, app_name, summary, body, icon, urgency, timestamp, source_id,
                    actions, hints, expire_timeout, state, group_key, can_reply
             FROM   notifications
             ORDER  BY timestamp DESC
             LIMIT  ? OFFSET ?",
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_notif).collect()
    }

    /// Full-text search across history matching summary, body, or app_name.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Notification>> {
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let rows = sqlx::query(
            "SELECT id, app_name, summary, body, icon, urgency, timestamp, source_id,
                    actions, hints, expire_timeout, state, group_key, can_reply
             FROM   notifications
             WHERE  lower(summary)  LIKE lower(?) ESCAPE '\\'
                OR  lower(body)     LIKE lower(?) ESCAPE '\\'
                OR  lower(app_name) LIKE lower(?) ESCAPE '\\'
             ORDER  BY timestamp DESC
             LIMIT  ?",
        )
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_notif).collect()
    }

    /// Delete all notification records from the database.
    pub async fn clear_history(&self) -> Result<()> {
        sqlx::query("DELETE FROM notifications")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a single notification record by id.
    pub async fn delete_notification(&self, id: u32) -> Result<()> {
        sqlx::query("DELETE FROM notifications WHERE id = ?")
            .bind(id as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Return the total number of records in the history database.
    pub async fn count_all(&self) -> Result<u32> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM notifications")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0 as u32)
    }

    /// Purge old records beyond `keep` most recent.
    #[allow(dead_code)]
    pub async fn prune(&self, keep: usize) -> Result<()> {
        sqlx::query(
            "DELETE FROM notifications WHERE id NOT IN (
                 SELECT id FROM notifications ORDER BY timestamp DESC LIMIT ?
             )",
        )
        .bind(keep as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn row_to_notif(row: &sqlx::sqlite::SqliteRow) -> Result<Notification> {
    let actions_str: String = row.try_get("actions")?;
    let hints_str:   String = row.try_get("hints")?;
    let urgency_raw: i64    = row.try_get("urgency")?;
    let state_raw:   i64    = row.try_get("state")?;
    let can_reply:   i64    = row.try_get("can_reply")?;

    let actions: Vec<NotificationAction> =
        serde_json::from_str(&actions_str).unwrap_or_default();
    let hints: std::collections::HashMap<String, String> =
        serde_json::from_str(&hints_str).unwrap_or_default();
    let reply_placeholder = hints
        .get("x-ember-reply-placeholder")
        .cloned()
        .unwrap_or_else(|| "Reply…".to_string());
    let max_reply_length = hints
        .get("x-ember-max-reply-length")
        .and_then(|v| v.parse::<u32>().ok());

    Ok(Notification {
        id:             row.try_get::<i64, _>("id")? as u32,
        app_name:       row.try_get("app_name")?,
        summary:        row.try_get("summary")?,
        body:           row.try_get("body")?,
        icon:           row.try_get("icon")?,
        urgency:        Urgency::from_u8(urgency_raw as u8),
        timestamp:      row.try_get("timestamp")?,
        source_id:      row.try_get::<i64, _>("source_id")? as u32,
        actions,
        hints,
        expire_timeout: row.try_get("expire_timeout")?,
        state:          NotificationState::from_i32(state_raw as i32),
        group_key:      row.try_get("group_key")?,
        can_reply:      can_reply != 0,
        reply_placeholder,
        max_reply_length,
    })
}

#[cfg(test)]
mod tests {
    use super::SqliteStore;
    use crate::store::models::{Notification, NotificationAction, NotificationState, Urgency};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_db_path(name: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("ember-{name}-{}-{ts}.db", std::process::id()))
    }

    fn cleanup_db(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}.wal", path.display())));
        let _ = std::fs::remove_file(PathBuf::from(format!("{}.shm", path.display())));
    }

    fn notif(id: u32, app: &str, summary: &str, body: &str, ts: i64) -> Notification {
        Notification {
            id,
            app_name: app.to_string(),
            summary: summary.to_string(),
            body: body.to_string(),
            icon: String::new(),
            urgency: Urgency::Normal,
            timestamp: ts,
            source_id: 0,
            actions: vec![NotificationAction {
                key: "default".to_string(),
                label: "Open".to_string(),
            }],
            hints: HashMap::new(),
            expire_timeout: -1,
            state: NotificationState::Active,
            group_key: Some(app.to_lowercase()),
            can_reply: false,
            reply_placeholder: "Reply…".to_string(),
            max_reply_length: None,
        }
    }

    #[tokio::test]
    async fn search_matches_summary_body_and_app_name_case_insensitive() {
        let path = test_db_path("search-fields");
        cleanup_db(&path);
        let store = SqliteStore::open(&path).await.expect("open db");

        store
            .upsert(&notif(1, "MailApp", "New mail", "inbox alert", 100))
            .await
            .expect("insert 1");
        store
            .upsert(&notif(2, "Chat", "Ping", "Contains KEYWORD in body", 200))
            .await
            .expect("insert 2");

        let by_summary = store.search("MAIL", 10).await.expect("search summary");
        assert_eq!(by_summary.len(), 1);
        assert_eq!(by_summary[0].id, 1);

        let by_body = store.search("keyword", 10).await.expect("search body");
        assert_eq!(by_body.len(), 1);
        assert_eq!(by_body[0].id, 2);

        let by_app = store.search("mailapp", 10).await.expect("search app");
        assert_eq!(by_app.len(), 1);
        assert_eq!(by_app[0].id, 1);

        cleanup_db(&path);
    }

    #[tokio::test]
    async fn search_escapes_sql_wildcards_and_respects_limit() {
        let path = test_db_path("search-escape-limit");
        cleanup_db(&path);
        let store = SqliteStore::open(&path).await.expect("open db");

        store
            .upsert(&notif(10, "A", "Disk 100% used", "first", 100))
            .await
            .expect("insert 10");
        store
            .upsert(&notif(11, "B", "Disk 100% used again", "second", 200))
            .await
            .expect("insert 11");
        store
            .upsert(&notif(12, "C", "Disk 100x used", "third", 300))
            .await
            .expect("insert 12");

        let pct = store.search("100%", 10).await.expect("search percent");
        assert_eq!(pct.len(), 2);
        assert!(pct.iter().all(|n| n.summary.contains("100%")));

        let limited = store.search("disk", 1).await.expect("search with limit");
        assert_eq!(limited.len(), 1);
        // Newest by timestamp should be returned first.
        assert_eq!(limited[0].id, 12);

        cleanup_db(&path);
    }
}
