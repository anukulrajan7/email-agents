-- Gmail connections (scope https://mail.google.com/) can save drafts and send on explicit user action.
UPDATE email_sources
SET capabilities_json = '{"read":true,"sync":true,"provider_labels":false,"provider_drafts":true,"send":true}'
WHERE kind = 'nango_gmail';
ALTER TABLE drafts ADD COLUMN provider_message_id TEXT;
