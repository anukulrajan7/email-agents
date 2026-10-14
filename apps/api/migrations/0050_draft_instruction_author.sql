-- Who said the words in drafts.instructions_by_user. Only that person's own sends count them as supported facts.
ALTER TABLE drafts ADD COLUMN instructions_user_id TEXT;
