-- Apply FORCE RLS to every public table with a recognized tenant key. Tables
-- carrying nullable legacy tenant keys remain fail-closed for NULL values.
DO $$
DECLARE
    t record;
    tenant_column text;
    policy_name text;
    covered_count bigint;
    missing_count bigint;
BEGIN
    FOR t IN
        SELECT c.oid, n.nspname, c.relname
          FROM pg_class c
          JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public'
           AND c.relkind IN ('r', 'p')
         ORDER BY c.relname
    LOOP
        IF EXISTS (SELECT 1 FROM public.rls_exempt_tables e WHERE e.table_name = t.relname) THEN
            CONTINUE;
        END IF;

        SELECT a.attname INTO tenant_column
          FROM pg_attribute a
         WHERE a.attrelid = t.oid
           AND a.attnum > 0
           AND NOT a.attisdropped
           AND a.attname IN ('organization_id', 'org_id', 'tenant_id')
         ORDER BY CASE a.attname
                    WHEN 'organization_id' THEN 1
                    WHEN 'org_id' THEN 2
                    ELSE 3
                  END
         LIMIT 1;

        IF tenant_column IS NULL THEN
            CONTINUE;
        END IF;

        EXECUTE format('ALTER TABLE %I.%I ENABLE ROW LEVEL SECURITY', t.nspname, t.relname);
        EXECUTE format('ALTER TABLE %I.%I FORCE ROW LEVEL SECURITY', t.nspname, t.relname);
        policy_name := 'tenant_org_isolation';
        EXECUTE format('DROP POLICY IF EXISTS %I ON %I.%I', policy_name, t.nspname, t.relname);
        EXECUTE format(
            'CREATE POLICY %I ON %I.%I USING (%I = app.current_org_id()) WITH CHECK (%I = app.current_org_id())',
            policy_name, t.nspname, t.relname, tenant_column, tenant_column
        );
    END LOOP;

    SELECT count(*) INTO missing_count
      FROM pg_class c
      JOIN pg_namespace n ON n.oid = c.relnamespace
      JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
     WHERE n.nspname = 'public'
       AND c.relkind IN ('r', 'p')
       AND a.attname IN ('organization_id', 'org_id', 'tenant_id')
       AND NOT EXISTS (SELECT 1 FROM public.rls_exempt_tables e WHERE e.table_name = c.relname)
       AND (
           NOT c.relrowsecurity
           OR NOT c.relforcerowsecurity
           OR NOT EXISTS (
               SELECT 1 FROM pg_policies p
                WHERE p.schemaname = n.nspname
                  AND p.tablename = c.relname
                  AND p.policyname = 'tenant_org_isolation'
           )
       );
    IF missing_count <> 0 THEN
        RAISE EXCEPTION 'tenant RLS coverage incomplete: % table(s) lack ENABLE/FORCE', missing_count;
    END IF;

    SELECT count(DISTINCT c.oid) INTO covered_count
      FROM pg_class c
      JOIN pg_namespace n ON n.oid = c.relnamespace
      JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
     WHERE n.nspname = 'public'
       AND c.relkind IN ('r', 'p')
       AND a.attname IN ('organization_id', 'org_id', 'tenant_id')
       AND NOT EXISTS (SELECT 1 FROM public.rls_exempt_tables e WHERE e.table_name = c.relname);
    RAISE NOTICE 'FORCE RLS enabled on % tenant table(s)', covered_count;
END $$;
