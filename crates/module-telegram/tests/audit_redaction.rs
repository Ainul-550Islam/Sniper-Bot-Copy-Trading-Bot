//! BATCH 04 — audit rows never contain nonce, token or key material.
//!
//! The audit trail is append-only and operator-readable, so it must be safe to
//! export: no confirmation nonce, no bot token, no signing key. This test drives
//! the whole happy path through the real dispatcher (with a real bot token in
//! the client) and then scans every audit row and every reply for the two
//! secrets that exist in the process: the nonce the chat received, and the bot
//! token used by the API client.

mod support;

use module_telegram::commands::audit::{scrub, AuditDetail, AuditDetailKeys};
use support::{extract_nonce, Bot, CHAT, LINKED_USER, ORG};

const BOT_TOKEN: &str = "test-token-not-real";
const MINT: &str = "So11111111111111111111111111111111111111112";

#[tokio::test]
async fn audit_rows_never_contain_the_nonce_or_the_token() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    let reply = bot
        .say(CHAT, LINKED_USER, &format!("/buy {MINT} 0.01 100"))
        .await;
    let nonce = extract_nonce(&reply).expect("nonce");
    bot.say(CHAT, LINKED_USER, &format!("/confirm {nonce}"))
        .await;
    // A denial too, so both the happy and the refused paths are covered.
    bot.say(CHAT, LINKED_USER, &format!("/buy {MINT} 9")).await;

    assert!(
        bot.stores
            .audit
            .recent(ORG, 500)
            .await
            .expect("audit")
            .len()
            >= 4
    );

    // 1. The rendered audit trail (the exact shape an export would take).
    let rendered = bot.stores.memory().expect("in-memory").audit.render();
    assert!(
        !rendered.contains(&nonce),
        "nonce leaked into audit: {rendered}"
    );
    assert!(!rendered.contains(BOT_TOKEN), "token leaked into audit");
    assert!(
        !rendered.contains("nonce"),
        "audit must not even carry the key name"
    );
    // It still carries the facts an operator needs.
    assert!(rendered.contains("\"reason\""), "{rendered}");
    assert!(rendered.contains("\"amount_lamports\""), "{rendered}");
    assert!(
        rendered.contains(MINT),
        "the mint is public data: {rendered}"
    );

    // 2. Every reply the chat received: the nonce appears ONLY in the reply
    //    that delivered it (that is its whole purpose), never twice.
    let texts = bot.fake.texts();
    let with_nonce = texts.iter().filter(|t| t.contains(&nonce)).count();
    assert_eq!(with_nonce, 1, "the nonce must be delivered exactly once");
    for text in &texts {
        assert!(
            !text.contains(BOT_TOKEN),
            "token leaked into a reply: {text}"
        );
    }

    // 3. The command row keeps only the digest.
    let rows = bot.stores.memory().expect("in-memory").commands.all();
    // Only the accepted request created a command row; the refused one was
    // denied (and audited) before any row could exist.
    assert_eq!(rows.len(), 1);
    for row in rows {
        let debug = format!("{row:?}");
        assert!(!debug.contains(&nonce), "nonce leaked into a command row");
        assert_eq!(row.nonce_sha256.len(), 32);
    }
    bot.stop().await;
}

#[test]
fn scrub_removes_secret_shapes_and_keeps_plain_words() {
    let nonce = "0123456789abcdef0123456789abcdef";
    let token = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
    let keypair = "5J8tGmYq3kR9wVbN2xPzL7cDfH4uEsA1nM6yT0iKoBvQ";
    let scrubbed = scrub(&format!(
        "port failed: nonce {nonce} bot {token} key {keypair} try again"
    ));
    assert!(!scrubbed.contains(nonce), "{scrubbed}");
    assert!(!scrubbed.contains(token), "{scrubbed}");
    assert!(!scrubbed.contains(keypair), "{scrubbed}");
    assert!(scrubbed.contains("port failed"), "{scrubbed}");
    assert!(scrubbed.contains("try again"), "{scrubbed}");
    // Plain operator text survives untouched.
    assert_eq!(scrub("max exposure exceeded"), "max exposure exceeded");
    // Control characters cannot smuggle anything through.
    assert_eq!(scrub("a\nb\tc"), "a b c");
    // Over-long input is truncated before hashing/logging.
    assert!(scrub(&"x".repeat(1_000)).len() <= 200);
}

#[test]
fn the_detail_builder_has_no_free_form_escape_hatch() {
    // Every key the builder can emit is on the whitelist; there is no method
    // taking an arbitrary key, so this list is the complete surface.
    let detail = AuditDetail::new();
    let keys = detail.as_map().keys().collect::<Vec<_>>();
    assert!(keys.is_empty());
    let whitelist = AuditDetailKeys::all();
    assert!(whitelist.contains(&"reason"));
    assert!(whitelist.contains(&"failure_detail"));
    assert!(whitelist.contains(&"slippage_bps"));
    assert!(!whitelist.iter().any(|k| k.contains("nonce")));
    assert!(!whitelist.iter().any(|k| k.contains("token")));
    assert!(!whitelist.iter().any(|k| k.contains("secret")));
    assert!(!whitelist.iter().any(|k| k.contains("key")));
}
