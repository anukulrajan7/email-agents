-- Labels remember which pack brought them, so turning a pack off can hide them and turning it on
-- can bring them back. User-created labels keep a NULL source_pack and are never touched by packs.
ALTER TABLE labels ADD COLUMN source_pack TEXT;
ALTER TABLE labels ADD COLUMN archived_by_pack INTEGER NOT NULL DEFAULT 0;

-- Existing rows: the first pack (catalogue order) that ships a label with that key. `other` stays NULL.
UPDATE labels SET source_pack = CASE key
  WHEN 'action_required' THEN 'general'
  WHEN 'question' THEN 'general'
  WHEN 'customer_support' THEN 'general'
  WHEN 'sales_lead' THEN 'general'
  WHEN 'billing' THEN 'general'
  WHEN 'scheduling' THEN 'general'
  WHEN 'fyi' THEN 'general'
  WHEN 'timesheet' THEN 'staffing'
  WHEN 'leave' THEN 'staffing'
  WHEN 'candidate_referral' THEN 'staffing'
  WHEN 'client_requirement' THEN 'staffing'
  WHEN 'approval' THEN 'staffing'
  WHEN 'payroll' THEN 'staffing'
  WHEN 'general' THEN 'staffing'
  WHEN 'bug_report' THEN 'support'
  WHEN 'complaint' THEN 'support'
  WHEN 'refund_request' THEN 'support'
  WHEN 'follow_up' THEN 'sales'
  WHEN 'invoice' THEN 'finance'
  WHEN 'payment' THEN 'finance'
  WHEN 'expense' THEN 'finance'
  WHEN 'important' THEN 'personal'
  WHEN 'needs_reply' THEN 'personal'
  WHEN 'newsletters' THEN 'personal'
  WHEN 'receipts' THEN 'personal'
  ELSE NULL
END;
