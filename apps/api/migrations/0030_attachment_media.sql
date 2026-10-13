-- Every attachment becomes text for the AI. The text itself reuses attachments.text_content / text_status
-- (0009); these columns say what kind of file it was, how reading went, and small facts for the UI.
-- Values are enforced in Rust (`attachments::Kind`, job outcomes), not by CHECKs, so they can grow.
--   extract_kind:   sheet | office | pdf | docx | image | audio | unsupported
--   extract_status: done | failed | skipped   (NULL = not read yet)
ALTER TABLE attachments ADD COLUMN extract_kind TEXT;
ALTER TABLE attachments ADD COLUMN extract_meta_json TEXT;
ALTER TABLE attachments ADD COLUMN extract_status TEXT;
ALTER TABLE attachments ADD COLUMN extract_error TEXT;

-- Hash of the attachment text the insight was written from, so new text triggers one refresh.
ALTER TABLE message_insights ADD COLUMN attachments_hash TEXT;
