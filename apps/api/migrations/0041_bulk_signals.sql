-- Header evidence that a message is bulk or automated (JSON array of signal names, see src/bulk.rs).
ALTER TABLE email_messages ADD COLUMN bulk_signals TEXT NOT NULL DEFAULT '[]';
