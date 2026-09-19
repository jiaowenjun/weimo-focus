CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    calendar_name TEXT NOT NULL,
    event_date TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    title TEXT NOT NULL,
    first_item INTEGER NOT NULL,
    last_item INTEGER NOT NULL,
    sync_marker TEXT NOT NULL,
    sync_status TEXT NOT NULL CHECK (sync_status IN ('pending', 'synced')),
    sync_attempts INTEGER NOT NULL DEFAULT 0 CHECK (sync_attempts >= 0),
    last_sync_error_code TEXT,
    last_sync_error TEXT,
    last_sync_action TEXT CHECK (
        last_sync_action IS NULL OR last_sync_action IN ('created', 'existing')
    ),
    last_attempted_at TEXT,
    synced_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (length(trim(calendar_name)) > 0),
    CHECK (length(trim(title)) > 0),
    CHECK (last_item >= first_item),
    CHECK (end_time > start_time),
    CHECK (
        (sync_status = 'pending' AND synced_at IS NULL)
        OR
        (sync_status = 'synced' AND synced_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS events_status_time_idx
    ON events (sync_status, event_date DESC, start_time DESC);

CREATE INDEX IF NOT EXISTS events_marker_idx
    ON events (sync_marker);

CREATE INDEX IF NOT EXISTS events_calendar_title_idx
    ON events (calendar_name, title, event_date DESC, start_time DESC);
