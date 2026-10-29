//! Source-level contract for the identity-plane SQL adjudication.
//!
//! The dynamic authorization and transaction behaviour of invitation
//! acceptance, email verification, and password reset remains covered by the
//! PostgreSQL suites in this directory, especially
//! `auth_postgres_concurrency.rs`,
//! `auth_identity_race_boundaries.rs`, and
//! `saas_auth_password_reset_pg.rs`. This suite protects the decision made
//! around those flows by the 2026-10-28 integration-cleanup batch:
//!
//! 1. Places where a durable invitation row supplies the organization must
//!    repeat that organization as a SQL predicate.
//! 2. The few statements that legitimately run before a trustworthy tenant
//!    context exists must match one exact normalized SQL fragment in the
//!    forensic scanner. They must not become a broad file/table exemption.
//! 3. The file stems used by the forensic scanner must stay unique, because
//!    a second Rust file with the same stem would otherwise inherit a
//!    statement-specific sanction accidentally.
//! 4. Password-reset completion must continue to use the canonical user
//!    loaded inside the reset transaction rather than issue a redundant
//!    global email lookup after commit.
//!
//! A source-level test is the right level for these invariants. Changing
//! one of the sanctioned statements is allowed, but it must deliberately
//! update the forensic scanner, `docs/FORENSIC-SQL-RESEARCH-2026.md`, and
//! these expectations in the same change after a fresh tenant-safety
//! review. Silent drift is what this file prevents.

use std::fs;
use std::path::{Path, PathBuf};

const AUTH_FLOWS: &str = "crates/server/src/saas/auth_flows.rs";
const EMAIL_VERIFICATION: &str = "crates/server/src/saas/email_verification.rs";
const PASSWORD_RESET: &str = "crates/server/src/saas/password_reset.rs";
const POSTGRES_IDENTITY: &str = "crates/server/src/saas/postgres.rs";
const FORENSIC_SCANNER: &str = "scripts/forensic-sql-scan.sh";

const INVITE_LOOKUP: &str = "SELECT id, organization_id, email, role, invited_by FROM invites WHERE token_hash = $1 AND status = 'pending' AND expires_at > now() FOR UPDATE";
const INVITED_USER_LOOKUP: &str = "SELECT id, record FROM saas_runtime_records WHERE kind = 'user' AND lookup_key = $1 FOR UPDATE";
const USERS_INSERT: &str = "INSERT INTO users (id, email, email_verified, display_name, password_hash, status, platform_admin, created_at, updated_at, last_login_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT (id) DO UPDATE SET email=EXCLUDED.email, email_verified=EXCLUDED.email_verified, display_name=EXCLUDED.display_name, password_hash=EXCLUDED.password_hash, status=EXCLUDED.status, platform_admin=EXCLUDED.platform_admin, updated_at=EXCLUDED.updated_at, last_login_at=EXCLUDED.last_login_at";
const EMAIL_VERIFICATION_LOOKUP: &str =
    "SELECT id, email_verified, status FROM users WHERE lower(email) = lower($1)";
const PASSWORD_RESET_LOOKUP: &str = "SELECT id, status FROM users WHERE lower(email) = lower($1)";

/// `(label, source path, scanner table, exact normalized SQL fragment)`.
const SANCTIONS: &[(&str, &str, &str, &str)] = &[
    (
        "invitation bearer-token lookup",
        AUTH_FLOWS,
        "invites",
        INVITE_LOOKUP,
    ),
    (
        "invited global-user lookup",
        AUTH_FLOWS,
        "saas_runtime_records",
        INVITED_USER_LOOKUP,
    ),
    (
        "invitation global-user projection",
        AUTH_FLOWS,
        "users",
        USERS_INSERT,
    ),
    (
        "canonical global-user projection",
        POSTGRES_IDENTITY,
        "users",
        USERS_INSERT,
    ),
    (
        "email-verification account lookup",
        EMAIL_VERIFICATION,
        "users",
        EMAIL_VERIFICATION_LOOKUP,
    ),
    (
        "password-reset account lookup",
        PASSWORD_RESET,
        "users",
        PASSWORD_RESET_LOOKUP,
    ),
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_repository_file(relative_path: &str) -> String {
    let path = repository_root().join(relative_path);
    match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => panic!("failed to read {}: {error}", path.display()),
    }
}

fn normalized(contents: &str) -> String {
    contents.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn position_of(haystack: &str, needle: &str, label: &str) -> usize {
    match haystack.find(needle) {
        Some(position) => position,
        None => panic!("missing {label}: {needle}"),
    }
}

fn source_stem(relative_path: &str) -> String {
    let path = Path::new(relative_path);
    match path.file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) => stem.to_owned(),
        None => panic!("source path has no UTF-8 file stem: {relative_path}"),
    }
}

fn collect_rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => panic!("failed to list {}: {error}", directory.display()),
    };
    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => panic!(
                "failed to read an entry below {}: {error}",
                directory.display()
            ),
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => panic!("failed to inspect {}: {error}", path.display()),
        };
        if file_type.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn identity_sanctions_match_exactly_one_source_and_scanner_statement() {
    let scanner = normalized(&read_repository_file(FORENSIC_SCANNER));

    for &(label, relative_path, table, fragment) in SANCTIONS {
        let source = normalized(&read_repository_file(relative_path));
        assert_eq!(
            source.matches(fragment).count(),
            1,
            "{label} must appear exactly once in {relative_path}"
        );

        let stem = source_stem(relative_path);
        let scanner_entry = format!("(\"{stem}\", \"{table}\", \"{fragment}\"");
        assert_eq!(
            scanner.matches(&scanner_entry).count(),
            1,
            "{label} must have exactly one statement-specific scanner sanction"
        );
    }
}

#[test]
fn identity_sanctions_never_degrade_into_broad_file_table_exemptions() {
    let scanner = normalized(&read_repository_file(FORENSIC_SCANNER));
    let identity_start = position_of(
        &scanner,
        "IDENTITY_PLANE_SANCTIONS = [",
        "identity-plane sanction list",
    );
    let before_identity_list = &scanner[..identity_start];
    let identity_and_later = &scanner[identity_start..];
    let identity_end = position_of(
        identity_and_later,
        "] GROUPS = [",
        "end of the identity-plane sanction list",
    );
    let identity_section = &identity_and_later[..identity_end];

    assert_eq!(
        identity_section.matches("(\"").count(),
        SANCTIONS.len(),
        "the scanner must contain exactly the adjudicated identity sanctions"
    );

    for &(label, relative_path, table, _fragment) in SANCTIONS {
        let stem = source_stem(relative_path);
        let broad_entry = format!("(\"{stem}\", \"{table}\"");
        assert!(
            !before_identity_list.contains(&broad_entry),
            "{label} must not be present in the broad SANCTIONED_GLOBAL list"
        );
        assert!(
            identity_section.contains(&broad_entry),
            "{label} is missing from the statement-specific identity list"
        );
    }
}

#[test]
fn scanner_identity_file_stems_are_unique_across_every_crate() {
    let mut rust_files = Vec::new();
    collect_rust_files(&repository_root().join("crates"), &mut rust_files);
    assert!(
        rust_files.len() > 700,
        "the recursive crate sweep unexpectedly found only {} Rust files",
        rust_files.len()
    );

    for &(_label, relative_path, _table, _fragment) in SANCTIONS {
        let expected_stem = source_stem(relative_path);
        let matching_files: Vec<&PathBuf> = rust_files
            .iter()
            .filter(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem == expected_stem)
            })
            .collect();
        assert_eq!(
            matching_files.len(),
            1,
            "scanner stem {expected_stem} must identify exactly one Rust file; found {matching_files:?}"
        );
        assert!(
            matching_files[0].ends_with(relative_path),
            "scanner stem {expected_stem} resolved to {}, expected {relative_path}",
            matching_files[0].display()
        );
    }
}

#[test]
fn invitation_flow_derives_org_once_then_scopes_every_later_statement() {
    let source = normalized(&read_repository_file(AUTH_FLOWS));

    let invite_lookup_at = position_of(&source, INVITE_LOOKUP, "invitation lookup");
    let organization_slug_at = position_of(
        &source,
        "SELECT record->>'slug' AS slug FROM saas_runtime_records WHERE kind = 'organization' AND id = $1 AND organization_id = $2",
        "organization-scoped slug lookup",
    );
    let invited_user_at = position_of(&source, INVITED_USER_LOOKUP, "invited-user lookup");
    let users_insert_at = position_of(&source, USERS_INSERT, "global-user projection insert");
    let membership_lookup_at = position_of(
        &source,
        "SELECT id FROM saas_runtime_records WHERE kind = 'membership' AND lookup_key = $1 AND organization_id = $2 AND user_id = $3 FOR UPDATE",
        "organization-and-user-scoped membership lookup",
    );
    let invite_closure_at = position_of(
        &source,
        "UPDATE invites SET status = 'accepted', accepted_at = now(), accepted_by = $1 WHERE id = $2 AND organization_id = $3 AND token_hash = $4 AND status = 'pending' AND expires_at > now()",
        "fully scoped invitation closure",
    );

    assert!(invite_lookup_at < organization_slug_at);
    assert!(organization_slug_at < invited_user_at);
    assert!(invited_user_at < users_insert_at);
    assert!(users_insert_at < membership_lookup_at);
    assert!(membership_lookup_at < invite_closure_at);
}

#[test]
fn invitation_sql_binds_the_row_derived_organization_and_user() {
    let source = normalized(&read_repository_file(AUTH_FLOWS));

    assert_eq!(
        source
            .matches(".bind(organization_id.to_string()) .bind(organization_uuid)")
            .count(),
        1,
        "the organization-record lookup must bind the row-derived organization twice: record id and organization_id column"
    );
    assert_eq!(
        source
            .matches(".bind(&membership_lookup) .bind(organization_uuid) .bind(user.id.as_uuid())")
            .count(),
        1,
        "the existing-membership lookup must bind lookup key, organization, and user in that order"
    );
    assert_eq!(
        source
            .matches(
                ".bind(user.id.as_uuid()) .bind(invitation_id) .bind(organization_uuid) .bind(&token_hash)"
            )
            .count(),
        1,
        "the invitation closure must bind accepting user, invitation, organization, and bearer-token hash in that order"
    );
}

#[test]
fn password_reset_alert_uses_the_canonical_user_loaded_in_the_transaction() {
    let source = normalized(&read_repository_file(PASSWORD_RESET));

    assert_eq!(
        source.matches(PASSWORD_RESET_LOOKUP).count(),
        1,
        "the pre-authentication reset request lookup remains the one sanctioned global users read"
    );
    assert!(
        !source.contains("SELECT email FROM users"),
        "password-reset completion must not reintroduce a redundant unscoped email lookup"
    );
    assert!(
        !source.contains("async fn user_email"),
        "the redundant password-reset email helper must stay removed"
    );
    assert!(
        !source.contains("user_email(db, user_id)"),
        "password-reset completion must not call the removed email helper"
    );

    let canonical_update_at = position_of(
        &source,
        "PostgresSaasRepo::update_user_in_transaction(&mut transaction, &user, now)",
        "canonical user update inside the reset transaction",
    );
    let commit_at = position_of(
        &source,
        "if let Err(error) = transaction.commit().await",
        "password-reset transaction commit",
    );
    let security_alert_at = position_of(
        &source,
        "templates::security_alert( &user.email",
        "security alert addressed to the canonical loaded user",
    );
    assert!(canonical_update_at < commit_at);
    assert!(commit_at < security_alert_at);
}
