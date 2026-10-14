-- Warnings and the routing preview are worked out when the spec changes, so reading a session costs no workspace queries.
ALTER TABLE designer_sessions ADD COLUMN warnings_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE designer_sessions ADD COLUMN routing_preview TEXT;
