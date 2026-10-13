-- Whether the stored body is the full message (vs a provider preview/snippet).
ALTER TABLE email_messages ADD COLUMN content_complete INTEGER NOT NULL DEFAULT 1;
UPDATE email_messages SET content_complete = 0
WHERE body_html IS NULL AND length(body_text) < 300
  AND source_id IN (SELECT id FROM email_sources WHERE kind = 'nango_gmail');
