-- scan_status never moved off 'pending'; it now follows extract_status (done | failed | skipped) when reading finishes.
UPDATE attachments SET scan_status = extract_status WHERE scan_status = 'pending' AND extract_status IS NOT NULL;
