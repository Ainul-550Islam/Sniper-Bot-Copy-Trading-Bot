-- 0022_checkout_url.sql — persist provider checkout URL durably for Stripe/Paddle

ALTER TABLE checkout_sessions ADD COLUMN IF NOT EXISTS checkout_url text;
CREATE INDEX IF NOT EXISTS checkout_sessions_checkout_url_idx ON checkout_sessions (checkout_url) WHERE checkout_url IS NOT NULL;
