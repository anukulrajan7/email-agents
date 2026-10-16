-- Slice S9.1: the headers that say which mail answers which. `in_reply_to` and `references_json` let a reply be matched to the
-- question we asked; `auth_json` keeps what the receiving server concluded about the sender (dmarc, spf, dkim); `auto_kind` marks
-- machine answers ('ooo' out-of-office, 'dsn' delivery failure). `drafts.rfc_message_id` is the Message-ID our own mail went out with,
-- so a reply to it can be recognised and our own copy in the Sent folder is not mistaken for a person's answer.
ALTER TABLE email_messages ADD COLUMN in_reply_to TEXT;
ALTER TABLE email_messages ADD COLUMN references_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE email_messages ADD COLUMN auth_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE email_messages ADD COLUMN auto_kind TEXT CHECK (auto_kind IN ('ooo', 'dsn'));
ALTER TABLE drafts ADD COLUMN rfc_message_id TEXT;
CREATE INDEX idx_drafts_rfc_message_id ON drafts(rfc_message_id) WHERE rfc_message_id IS NOT NULL;
