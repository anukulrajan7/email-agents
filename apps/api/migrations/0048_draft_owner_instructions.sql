-- What the owner themselves asked the draft to say. It counts as a supported fact in send checks.
ALTER TABLE drafts ADD COLUMN instructions_by_user TEXT;
