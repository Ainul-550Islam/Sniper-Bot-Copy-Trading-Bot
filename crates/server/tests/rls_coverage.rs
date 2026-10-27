use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

fn postgres_url() -> String {
    std::env::var("POSTGRES_URL")
        .expect("POSTGRES_URL is required for the PostgreSQL RLS coverage test")
}

#[tokio::test]
async fn every_tenant_keyed_public_table_has_forced_rls_or_a_reasoned_exemption() {
    if std::env::var("REQUIRE_POSTGRES").ok().as_deref() == Some("1") {
        assert!(
            std::env::var_os("POSTGRES_URL").is_some(),
            "REQUIRE_POSTGRES=1 requires POSTGRES_URL"
        );
    }
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&postgres_url())
        .await
        .expect("connect to PostgreSQL");
    bot_core::db::MIGRATOR
        .run(&pool)
        .await
        .expect("apply migrations");

    for (role, expected_bypass) in [("bot_app", false), ("bot_system", true)] {
        let row = sqlx::query(
            "SELECT rolcanlogin, rolsuper, rolbypassrls FROM pg_roles WHERE rolname = $1",
        )
        .bind(role)
        .fetch_optional(&pool)
        .await
        .expect("read RLS role metadata")
        .unwrap_or_else(|| panic!("required RLS role {role} is missing"));
        assert!(
            !row.get::<bool, _>("rolcanlogin"),
            "{role} must not accept direct logins"
        );
        assert!(
            !row.get::<bool, _>("rolsuper"),
            "{role} must not be a superuser"
        );
        assert_eq!(
            row.get::<bool, _>("rolbypassrls"),
            expected_bypass,
            "{role} RLS bypass configuration"
        );
    }
    let app_can_assume_system: bool =
        sqlx::query_scalar("SELECT pg_has_role('bot_app', 'bot_system', 'MEMBER')")
            .fetch_one(&pool)
            .await
            .expect("verify app role cannot assume the system role");
    assert!(
        !app_can_assume_system,
        "bot_app must never inherit bot_system privileges"
    );

    let rows = sqlx::query(
        "SELECT c.relname,
                c.relrowsecurity AS rls_enabled,
                c.relforcerowsecurity AS rls_forced,
                p.qual AS policy_using,
                p.with_check AS policy_check,
                e.reason AS exemption_reason
           FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
           JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
           LEFT JOIN pg_policies p ON p.schemaname = n.nspname
                                  AND p.tablename = c.relname
                                  AND p.policyname = 'tenant_org_isolation'
           LEFT JOIN public.rls_exempt_tables e ON e.table_name = c.relname
          WHERE n.nspname = 'public'
            AND c.relkind IN ('r', 'p')
            AND a.attname IN ('organization_id', 'org_id', 'tenant_id')
          GROUP BY c.oid, n.nspname, c.relname, c.relrowsecurity, c.relforcerowsecurity,
                   p.qual, p.with_check, e.reason
          ORDER BY c.relname",
    )
    .fetch_all(&pool)
    .await
    .expect("read tenant-table RLS metadata");
    assert!(
        !rows.is_empty(),
        "expected tenant-keyed tables in the migrated schema"
    );

    for row in rows {
        let table: String = row.get("relname");
        let enabled: bool = row.get("rls_enabled");
        let forced: bool = row.get("rls_forced");
        let policy_using: Option<String> = row.get("policy_using");
        let policy_check: Option<String> = row.get("policy_check");
        let reason: Option<String> = row.get("exemption_reason");
        let policy_valid = policy_using
            .as_deref()
            .is_some_and(|expression| expression.contains("current_org_id"))
            && policy_check
                .as_deref()
                .is_some_and(|expression| expression.contains("current_org_id"));
        assert!(
            (enabled && forced && policy_valid)
                || reason.as_deref().is_some_and(|value| !value.trim().is_empty()),
            "public.{table} has a tenant key but lacks ENABLE + FORCE RLS, USING/WITH CHECK tenant policies, and a documented exemption"
        );
    }
    pool.close().await;
}
