-- The account an audit entry is about, so readers can link to it; `target` is its username.
ALTER TABLE audit_log ADD COLUMN target_account INTEGER;

-- The API logged these with the account id as target.
UPDATE audit_log SET target_account = CAST(target AS INTEGER)
WHERE action IN ('account-ban', 'account-unban', 'account-rename', 'account-gm', 'account-passwd', 'kick')
  AND target <> '' AND target NOT GLOB '*[^0-9]*';
UPDATE audit_log SET target = (SELECT username FROM accounts WHERE id = audit_log.target_account)
WHERE target_account IS NOT NULL AND EXISTS (SELECT 1 FROM accounts WHERE id = audit_log.target_account);

-- Everything else logged the username.
UPDATE audit_log SET target_account = (SELECT id FROM accounts WHERE username = audit_log.target)
WHERE target_account IS NULL AND target IS NOT NULL
  AND action IN ('account-create', 'account-delete', 'account-ban', 'account-unban', 'account-rename', 'account-gm',
                 'account-passwd', 'register', 'set-first-password', 'auto-register');
