-- Connected-mailbox (Nango) bookkeeping.
ALTER TABLE email_sources ADD COLUMN provider_config_key TEXT;
ALTER TABLE email_sources ADD COLUMN sync_cursor TEXT;
ALTER TABLE email_sources ADD COLUMN last_error TEXT;
ALTER TABLE email_sources ADD COLUMN connected_by_user_id TEXT REFERENCES users(id);
