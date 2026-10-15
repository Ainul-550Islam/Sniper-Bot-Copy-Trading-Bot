-- 0058_telegram_chat_prefs.sql — per-chat alert preferences for the Telegram
-- control bot (File 3 §6.4). Additive only.
--
-- One row per (organization, chat, alert kind). A missing row means the
-- default: enabled. Rows are written only by the owning organization's
-- /alerts command, so a chat can never change another tenant's preferences.
--
-- ROLLBACK (manual): DROP TABLE IF EXISTS telegram_chat_prefs;

CREATE TABLE IF NOT EXISTS telegram_chat_prefs (
    organization_id  uuid NOT NULL,
    chat_id          bigint NOT NULL,
    alert_kind       text NOT NULL,
    enabled          boolean NOT NULL DEFAULT true,
    updated_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, chat_id, alert_kind)
);
