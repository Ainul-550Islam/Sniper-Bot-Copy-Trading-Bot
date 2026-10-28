//! BATCH 04 — the pre-existing control commands are unchanged.
//!
//! BATCH 04 only *adds* routing: when no [`module_telegram::TradeConsole`] is
//! attached, every old command must behave exactly as before (same text, same
//! state changes, same refusals), and the new commands must answer "not
//! configured" instead of failing silently or panicking. When a console *is*
//! attached, the old commands must still work untouched.

mod support;

use support::{Bot, CHAT, LINKED_USER, LINKED_USER as USER};

#[tokio::test]
async fn control_commands_work_without_a_trade_console() {
    let bot = Bot::start(false).await;

    // /status reports module + kill-switch state.
    let reply = bot.say(CHAT, USER, "/status").await;
    assert!(reply.contains("Status — mode:"), "{reply}");
    assert!(reply.contains("kill: off"), "{reply}");

    // /on and /off toggle modules and say so.
    let reply = bot.say(CHAT, USER, "/off sniper").await;
    assert!(reply.contains("sniper"), "{reply}");
    assert!(reply.contains("disabled"), "{reply}");
    assert!(
        !bot.state
            .is_enabled(bot_core::models::BotModule::Sniper)
            .await
    );
    let reply = bot.say(CHAT, USER, "/on sniper").await;
    assert!(reply.contains("enabled"), "{reply}");
    assert!(
        bot.state
            .is_enabled(bot_core::models::BotModule::Sniper)
            .await
    );

    // /kill engages the emergency stop; /resume clears it.
    let reply = bot.say(CHAT, USER, "/kill").await;
    assert!(reply.contains("KILL SWITCH ENGAGED"), "{reply}");
    assert!(bot.state.kill_switch());
    let reply = bot.say(CHAT, USER, "/resume").await;
    assert!(reply.contains("Kill switch cleared"), "{reply}");
    assert!(!bot.state.kill_switch());

    // Read-only views still answer.
    for (command, needle) in [
        ("/positions", "open position"),
        ("/trades", "No trades yet"),
        ("/pnl", "PnL"),
        ("/balance", "Balances"),
        ("/mode", "Execution mode"),
        ("/config", "Config"),
        ("/wallets", "Wallets"),
    ] {
        let reply = bot.say(CHAT, USER, command).await;
        assert!(
            reply.contains(needle),
            "{command} => {reply:?} should contain {needle:?}"
        );
    }

    // /help still lists the original commands.
    let reply = bot.say(CHAT, USER, "/help").await;
    for needle in [
        "/status", "/on", "/off", "/kill", "/resume", "/buy", "/sell",
    ] {
        assert!(reply.contains(needle), "help missing {needle}: {reply}");
    }

    // /mode live still switches the mode (the second gate stays closed).
    let reply = bot.say(CHAT, USER, "/mode live").await;
    assert!(reply.contains("LIVE") || reply.contains("live"), "{reply}");
    assert_eq!(
        bot.state.execution_mode().await,
        bot_core::models::ExecutionMode::Live
    );

    // The new commands degrade honestly without a console.
    for command in [
        "/confirm 0123456789abcdef0123456789abcdef",
        "/cancel 0123456789abcdef0123456789abcdef",
        "/pending",
    ] {
        let reply = bot.say(CHAT, USER, command).await;
        assert!(reply.contains("not configured"), "{command} => {reply:?}");
    }
    // /consent without a console is refused, not answered as "unknown command".
    let reply = bot.say(CHAT, USER, "/consent status").await;
    assert!(reply.contains("not configured"), "{reply}");
    // /buy without a console keeps the pre-existing notice (no trade desk, no
    // console) — byte-for-byte the old behaviour.
    let reply = bot
        .say(
            CHAT,
            USER,
            "/buy So11111111111111111111111111111111111111112 0.01",
        )
        .await;
    assert!(reply.contains("trading is not configured"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn unauthorized_chats_are_still_refused_verbatim() {
    let bot = Bot::start(false).await;
    // 12345 is not in the allowlist.
    let reply = bot.say(12_345, 6_789, "/status").await;
    assert!(
        reply.contains("not authorized to control this bot"),
        "{reply}"
    );
    assert!(!bot.state.kill_switch());
    bot.stop().await;
}

#[tokio::test]
async fn an_unknown_command_is_still_reported_as_unknown() {
    let bot = Bot::start(false).await;
    let reply = bot.say(CHAT, USER, "/frobnicate").await;
    assert!(reply.contains("Unknown command"), "{reply}");
    assert!(reply.contains("frobnicate"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn role_requirements_are_unchanged_for_control_commands() {
    let bot = Bot::start(false).await;
    bot.state
        .update_config(|cfg| {
            cfg.telegram.owner_user_ids = vec![LINKED_USER];
            cfg.telegram.readonly_user_ids = vec![900];
            cfg.telegram.allowed_user_ids = vec![901];
        })
        .await;
    // A readonly user may read but not kill.
    let reply = bot.say(CHAT, 900, "/status").await;
    assert!(reply.contains("Status"), "{reply}");
    let reply = bot.say(CHAT, 900, "/kill").await;
    assert!(reply.contains("cannot run this command"), "{reply}");
    assert!(!bot.state.kill_switch());
    // An operator may kill but may not switch to live.
    let reply = bot.say(CHAT, 901, "/kill").await;
    assert!(reply.contains("KILL SWITCH ENGAGED"), "{reply}");
    let reply = bot.say(CHAT, 901, "/mode live").await;
    assert!(reply.contains("cannot run this command"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn existing_commands_still_work_next_to_the_console() {
    let bot = Bot::start(true).await;
    bot.grant_consent().await;
    // /status, /kill and friends are untouched by the console.
    let reply = bot.say(CHAT, USER, "/status").await;
    assert!(reply.contains("Status — mode:"), "{reply}");
    let reply = bot.say(CHAT, USER, "/off copy").await;
    assert!(reply.contains("disabled"), "{reply}");
    let reply = bot.say(CHAT, USER, "/on copy").await;
    assert!(reply.contains("enabled"), "{reply}");
    // And the console commands are routed.
    let reply = bot.say(CHAT, USER, "/consent status").await;
    assert!(reply.contains("status: active"), "{reply}");
    bot.stop().await;
}

#[tokio::test]
async fn the_command_menu_gained_the_new_commands_only() {
    let menu = module_telegram::menu();
    let names: Vec<&str> = menu.iter().map(|c| c.command.as_str()).collect();
    for existing in [
        "status",
        "on",
        "off",
        "kill",
        "resume",
        "positions",
        "trades",
        "pnl",
        "balance",
        "mode",
        "config",
        "buy",
        "sell",
        "snipe",
        "wallets",
        "limit",
        "help",
    ] {
        assert!(names.contains(&existing), "menu lost {existing}");
    }
    for added in ["consent", "confirm", "pending"] {
        assert!(names.contains(&added), "menu missing {added}");
    }
    // Descriptions are non-empty and short enough for Telegram's 256 limit.
    for command in &menu {
        assert!(!command.description.is_empty());
        assert!(command.description.chars().count() <= 256);
    }
}
