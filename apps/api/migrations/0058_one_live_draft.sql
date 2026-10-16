-- One live (unsent) reply draft per message. Candidate-watch follow-ups (origin_key 'watch:...') are outside the rule.
-- Where a message has several, keep one: a scheduled or sending reply first, then a draft a person edited or approved,
-- then the newest; the rest are deleted (their owner words go with them). Then the database enforces it.
DELETE FROM drafts
WHERE status != 'sent'
  AND (origin_key IS NULL OR substr(origin_key, 1, 6) != 'watch:')
  AND id != (
    SELECT k.id FROM drafts k
    WHERE k.message_id = drafts.message_id
      AND k.status != 'sent'
      AND (k.origin_key IS NULL OR substr(k.origin_key, 1, 6) != 'watch:')
    ORDER BY (k.send_at IS NOT NULL OR k.send_attempt_id IS NOT NULL) DESC,
             (k.status IN ('edited', 'approved')) DESC,
             k.created_at DESC, k.rowid DESC
    LIMIT 1
  );

CREATE UNIQUE INDEX uq_drafts_live ON drafts(message_id)
  WHERE status != 'sent' AND (origin_key IS NULL OR substr(origin_key, 1, 6) != 'watch:');
