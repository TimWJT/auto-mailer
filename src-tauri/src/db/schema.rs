//! Versioned migrations. Each entry runs once, in order, inside a transaction.
//! Never edit a migration that has shipped; add a new one instead.

pub const MIGRATIONS: &[&str] = &[
    // 1 — initial schema
    r#"
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE accounts (
    id                 INTEGER PRIMARY KEY,
    provider           TEXT NOT NULL CHECK (provider IN ('gmail', 'microsoft')),
    email              TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name       TEXT NOT NULL DEFAULT '',
    account_type       TEXT NOT NULL,
    daily_limit        INTEGER NOT NULL,
    status             TEXT NOT NULL DEFAULT 'connected',
    status_message     TEXT,
    paused             INTEGER NOT NULL DEFAULT 0,
    limit_paused_until TEXT,
    next_send_at       TEXT,
    signature          TEXT NOT NULL DEFAULT '',
    poll_state         TEXT,
    last_polled_at     TEXT,
    connected_at       TEXT NOT NULL,
    created_at         TEXT NOT NULL
);

CREATE TABLE contacts (
    id         INTEGER PRIMARY KEY,
    email      TEXT NOT NULL UNIQUE,
    first_name TEXT NOT NULL DEFAULT '',
    last_name  TEXT NOT NULL DEFAULT '',
    fields     TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE lists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at TEXT NOT NULL
);

CREATE TABLE list_members (
    list_id    INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    PRIMARY KEY (list_id, contact_id)
);

CREATE TABLE tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE contact_tags (
    contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (contact_id, tag_id)
);

CREATE TABLE do_not_contact (
    email    TEXT PRIMARY KEY,
    reason   TEXT NOT NULL,
    added_at TEXT NOT NULL
);

CREATE TABLE campaigns (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    account_id   INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
    list_id      INTEGER REFERENCES lists(id) ON DELETE SET NULL,
    status       TEXT NOT NULL DEFAULT 'draft',
    window_days  TEXT NOT NULL DEFAULT '1,2,3,4,5',
    window_start INTEGER NOT NULL DEFAULT 540,
    window_end   INTEGER NOT NULL DEFAULT 1020,
    gap_min_secs INTEGER NOT NULL DEFAULT 45,
    gap_max_secs INTEGER NOT NULL DEFAULT 120,
    footer       TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    finished_at  TEXT
);

CREATE TABLE campaign_steps (
    id          INTEGER PRIMARY KEY,
    campaign_id INTEGER NOT NULL REFERENCES campaigns(id) ON DELETE CASCADE,
    step_number INTEGER NOT NULL,
    delay_days  INTEGER NOT NULL DEFAULT 0,
    subject     TEXT NOT NULL DEFAULT '',
    body        TEXT NOT NULL DEFAULT '',
    same_thread INTEGER NOT NULL DEFAULT 1,
    UNIQUE (campaign_id, step_number)
);

CREATE TABLE step_attachments (
    id          INTEGER PRIMARY KEY,
    step_id     INTEGER NOT NULL REFERENCES campaign_steps(id) ON DELETE CASCADE,
    file_name   TEXT NOT NULL,
    stored_path TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL
);

CREATE TABLE enrolments (
    id           INTEGER PRIMARY KEY,
    campaign_id  INTEGER NOT NULL REFERENCES campaigns(id) ON DELETE CASCADE,
    contact_id   INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    current_step INTEGER NOT NULL DEFAULT 1,
    status       TEXT NOT NULL DEFAULT 'queued',
    next_due_at  TEXT,
    stop_reason  TEXT,
    last_sent_at TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE (campaign_id, contact_id)
);
CREATE INDEX enrolments_due ON enrolments (status, next_due_at);

-- One row per (enrolment, step), ever. The UNIQUE constraint is what makes
-- "never send the same step to the same contact twice" hold even if the
-- scheduler logic has a bug.
CREATE TABLE outbound (
    id                  INTEGER PRIMARY KEY,
    enrolment_id        INTEGER NOT NULL REFERENCES enrolments(id) ON DELETE CASCADE,
    step_number         INTEGER NOT NULL,
    account_id          INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
    message_id          TEXT NOT NULL UNIQUE,
    to_email            TEXT NOT NULL,
    subject             TEXT NOT NULL,
    new_thread          INTEGER NOT NULL DEFAULT 1,
    status              TEXT NOT NULL,
    attempts            INTEGER NOT NULL DEFAULT 0,
    provider_message_id TEXT,
    thread_id           TEXT,
    internet_message_id TEXT,
    conversation_id     TEXT,
    web_link            TEXT,
    dry_run             INTEGER NOT NULL DEFAULT 0,
    attempted_at        TEXT NOT NULL,
    sent_at             TEXT,
    error               TEXT,
    UNIQUE (enrolment_id, step_number)
);
CREATE INDEX outbound_imid ON outbound (internet_message_id);
CREATE INDEX outbound_conv ON outbound (conversation_id);
CREATE INDEX outbound_to ON outbound (to_email);

CREATE TABLE inbound (
    id             INTEGER PRIMARY KEY,
    account_id     INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
    provider_id    TEXT NOT NULL,
    message_id     TEXT,
    from_email     TEXT NOT NULL,
    from_name      TEXT NOT NULL DEFAULT '',
    subject        TEXT NOT NULL DEFAULT '',
    received_at    TEXT NOT NULL,
    classification TEXT NOT NULL,
    outbound_id    INTEGER REFERENCES outbound(id) ON DELETE SET NULL,
    enrolment_id   INTEGER REFERENCES enrolments(id) ON DELETE SET NULL,
    contact_id     INTEGER REFERENCES contacts(id) ON DELETE SET NULL,
    campaign_id    INTEGER REFERENCES campaigns(id) ON DELETE SET NULL,
    snippet        TEXT NOT NULL DEFAULT '',
    body_text      TEXT NOT NULL DEFAULT '',
    web_link       TEXT,
    is_read        INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL,
    UNIQUE (account_id, provider_id)
);

CREATE TABLE send_log (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    sent_at    TEXT NOT NULL,
    kind       TEXT NOT NULL
);
CREATE INDEX send_log_account ON send_log (account_id, sent_at);
"#,
];
