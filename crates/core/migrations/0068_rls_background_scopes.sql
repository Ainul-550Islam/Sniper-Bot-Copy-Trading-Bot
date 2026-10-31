-- The existing bot_app policies and bot_system role already cover workers.
-- ROLLBACK: COMMENT ON FUNCTION app.current_org_id() IS NULL;
COMMENT ON FUNCTION app.current_org_id() IS
  'Transaction-local tenant identity for application requests and scoped background workers.';
