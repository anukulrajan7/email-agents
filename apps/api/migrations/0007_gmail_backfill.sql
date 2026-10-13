-- Direct Gmail backfill (via Nango proxy) runs once per connected source.
ALTER TABLE email_sources ADD COLUMN backfilled_at INTEGER;
