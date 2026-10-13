-- Re-prioritise jobs already waiting: stages for mail from the last 48 h first, older mail last.
UPDATE jobs SET priority = CASE
    WHEN (SELECT m.received_at FROM email_messages m WHERE m.id = json_extract(jobs.payload_json, '$.message_id'))
         >= (CAST(strftime('%s', 'now') AS INTEGER) - 48 * 3600) * 1000 THEN 3
    ELSE -3 END
WHERE status = 'pending' AND priority = 0 AND json_extract(payload_json, '$.message_id') IS NOT NULL
  AND dedupe_key NOT LIKE '%:recheck:%';
