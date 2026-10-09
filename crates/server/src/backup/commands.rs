//! CLI command helpers for export/restore operations (Batch 5).
//! Safe pg_dump/pg_restore command builders — never embed secrets in returned strings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafeCommand {
    pub program: String,
    pub args_redacted: Vec<String>,
    pub detail: String,
}

impl SafeCommand {
    pub fn new(
        program: impl Into<String>,
        args_redacted: Vec<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            program: program.into(),
            args_redacted,
            detail: detail.into(),
        }
    }
    pub fn display(&self) -> String {
        format!("{} {}", self.program, self.args_redacted.join(" "))
    }

    pub fn is_safe(&self) -> bool {
        let s = self.display().to_ascii_lowercase();
        !s.contains("password") && !s.contains("secret") && !s.contains("-----begin")
    }
}

pub fn pg_dump_command(
    database_url_env: &str,
    output_path: &str,
    tenant_id: Option<&str>,
) -> SafeCommand {
    let mut args = vec!["--format=custom".into(), format!("--file={output_path}")];
    if let Some(t) = tenant_id {
        args.push(format!("--table=tenant_data_{t}"));
    }
    SafeCommand::new(
        "pg_dump",
        args,
        format!(
            "pg_dump via {database_url_env} env var (never inline URL) — tenant={:?}",
            tenant_id
        ),
    )
}

pub fn pg_restore_command(database_url_env: &str, input_path: &str) -> SafeCommand {
    SafeCommand::new(
        "pg_restore",
        vec![
            "--clean".into(),
            "--if-exists".into(),
            format!("--dbname=env:{database_url_env}"),
            input_path.into(),
        ],
        format!("pg_restore into {database_url_env}"),
    )
}

pub fn redis_export_command(
    redis_url_env: &str,
    output_path: &str,
    prefix: Option<&str>,
) -> SafeCommand {
    let mut args = vec![format!("--out={output_path}")];
    if let Some(p) = prefix {
        args.push(format!("--prefix={p}*"));
    }
    SafeCommand::new(
        "redis-dump",
        args,
        format!(
            "redis export via {redis_url_env} env var prefix={:?}",
            prefix
        ),
    )
}

pub fn verify_sha256_command(file_path: &str, expected_sha256: &str) -> SafeCommand {
    SafeCommand::new(
        "sha256sum",
        vec![file_path.into()],
        format!("verify sha256 {expected_sha256} for {file_path}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_dump_never_embeds_secret() {
        let c = pg_dump_command("DATABASE_URL", "/tmp/out.dump", Some("t1"));
        assert!(c.is_safe());
        assert!(!c.display().contains("postgres://"));
        assert!(c.display().contains("pg_dump"));
    }

    #[test]
    fn pg_restore_uses_env() {
        let c = pg_restore_command("DATABASE_URL", "/tmp/in.dump");
        assert!(c.display().contains("env:DATABASE_URL"));
        assert!(c.is_safe());
    }

    #[test]
    fn redis_export_safe() {
        let c = redis_export_command("REDIS_URL", "/tmp/redis.rdb", Some("tenant:t1:"));
        assert!(c.is_safe());
        assert!(c.display().contains("redis-dump"));
    }

    #[test]
    fn sha_verify_has_expected() {
        let sha = "a".repeat(64);
        let c = verify_sha256_command("/tmp/file", &sha);
        assert!(c.detail.contains(&sha));
    }
}
