//! Transactional email templates (GAP-MAP v2 P1).
//!
//! Five templates exist: email verification, password reset, member invite,
//! security alert, invoice receipt. Rendering is pure string composition —
//! no template engine dependency, no panics, and every user-supplied value
//! is HTML-escaped before it reaches an HTML body. Tokens appear ONLY as
//! part of the link the caller composes; this module never sees the raw
//! token, only the already-built URL.

/// A fully rendered email ready for the outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Stable template identifier (matches the `email_outbox.template_key`
    /// CHECK constraint).
    pub template_key: &'static str,
    pub subject: String,
    pub body_text: String,
    pub body_html: String,
}

/// Escape a value for safe interpolation into an HTML body or attribute.
fn esc(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Minimal shared HTML wrapper. Deliberately simple and inline-styled:
/// transactional mail clients strip most CSS, and we never load remote
/// assets from an email.
fn wrap(title: &str, body_html: &str) -> String {
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{title}</title></head>\n<body style=\"font-family:Arial,Helvetica,sans-serif;color:#1a1a1a;background:#f6f7f9;margin:0;padding:24px;\">\n<div style=\"max-width:560px;margin:0 auto;background:#ffffff;border:1px solid #e3e6ea;border-radius:8px;padding:24px;\">\n{body_html}\n<hr style=\"border:none;border-top:1px solid #e3e6ea;margin:20px 0;\">\n<p style=\"font-size:12px;color:#6b7280;\">Sniper-Suite — automated message. Do not reply. If you did not request this, you can safely ignore it.</p>\n</div>\n</body></html>\n",
        title = esc(title)
    )
}

fn button(label: &str, href: &str) -> String {
    format!(
        "<p style=\"margin:20px 0;\"><a href=\"{href}\" style=\"display:inline-block;background:#111827;color:#ffffff;text-decoration:none;padding:10px 18px;border-radius:6px;font-weight:bold;\">{label}</a></p>",
        href = esc(href),
        label = esc(label)
    )
}

/// Email-verification message with a single-use link.
pub fn email_verification(to: &str, link: &str, expiry_minutes: u32) -> Rendered {
    let _ = to; // recipient lives on the envelope, kept for symmetry
    let subject = "Verify your Sniper-Suite email address".to_string();
    let text = format!(
        "Confirm this email address to finish setting up your Sniper-Suite account.\n\nOpen this link to verify (expires in {expiry_minutes} minutes):\n{link}\n\nIf you did not create a Sniper-Suite account, ignore this email.",
        link = link
    );
    let html = wrap(
        "Verify your email",
        &format!(
            "<h2 style=\"margin-top:0;\">Verify your email</h2>\n<p>Confirm this address to finish setting up your Sniper-Suite account. The link expires in {expiry_minutes} minutes.</p>\n{btn}\n<p style=\"font-size:13px;color:#374151;\">Or paste this URL into your browser:<br><code>{link}</code></p>\n<p style=\"font-size:13px;\">If you did not create an account, ignore this email — nothing happens unless the link is used.</p>",
            btn = button("Verify email address", link),
            link = esc(link)
        ),
    );
    Rendered {
        template_key: "email_verification",
        subject,
        body_text: text,
        body_html: html,
    }
}

/// Password-reset message with a single-use link. The wording is identical
/// for known and unknown accounts (callers decide; this template is neutral
/// so no template change can leak account existence).
pub fn password_reset(to: &str, link: &str, expiry_minutes: u32) -> Rendered {
    let _ = to;
    let subject = "Reset your Sniper-Suite password".to_string();
    let text = format!(
        "A password reset was requested for your Sniper-Suite account.\n\nOpen this link to choose a new password (expires in {expiry_minutes} minutes):\n{link}\n\nIf you did not request a reset, ignore this email — your password is unchanged.",
        link = link
    );
    let html = wrap(
        "Reset your password",
        &format!(
            "<h2 style=\"margin-top:0;\">Reset your password</h2>\n<p>A reset was requested for your account. The link expires in {expiry_minutes} minutes and can be used once.</p>\n{btn}\n<p style=\"font-size:13px;color:#374151;\">Or paste this URL into your browser:<br><code>{link}</code></p>\n<p style=\"font-size:13px;\">If you did not request this, ignore this email — your password stays unchanged.</p>",
            btn = button("Choose a new password", link),
            link = esc(link)
        ),
    );
    Rendered {
        template_key: "password_reset",
        subject,
        body_text: text,
        body_html: html,
    }
}

/// Organization invite.
pub fn member_invite(
    to: &str,
    inviter_name: &str,
    organization_name: &str,
    link: &str,
    role: &str,
) -> Rendered {
    let _ = to;
    let subject = format!("Join {organization_name} on Sniper-Suite");
    let text = format!(
        "{inviter_name} invited you to join \"{organization_name}\" on Sniper-Suite as {role}.\n\nAccept the invitation:\n{link}\n\nIf you were not expecting this invitation, ignore this email.",
        inviter_name = inviter_name,
        organization_name = organization_name,
        role = role,
        link = link
    );
    let html = wrap(
        "You are invited",
        &format!(
            "<h2 style=\"margin-top:0;\">You are invited</h2>\n<p><strong>{inviter}</strong> invited you to join <strong>{org}</strong> on Sniper-Suite as <strong>{role}</strong>.</p>\n{btn}\n<p style=\"font-size:13px;color:#374151;\">Or paste this URL into your browser:<br><code>{link}</code></p>",
            inviter = esc(inviter_name),
            org = esc(organization_name),
            role = esc(role),
            btn = button("Accept invitation", link),
            link = esc(link)
        ),
    );
    Rendered {
        template_key: "member_invite",
        subject,
        body_text: text,
        body_html: html,
    }
}

/// Security alert (new login, MFA change, IP allowlist change, ...).
pub fn security_alert(to: &str, event: &str, detail: &str) -> Rendered {
    let _ = to;
    let subject = "Security notice from Sniper-Suite".to_string();
    let text = format!(
        "We recorded a security event on your Sniper-Suite account.\n\nEvent: {event}\nDetail: {detail}\n\nIf this was you, no action is needed. If it was not, change your password immediately and review your sessions and API keys.",
        event = event,
        detail = detail
    );
    let html = wrap(
        "Security notice",
        &format!(
            "<h2 style=\"margin-top:0;\">Security notice</h2>\n<p>We recorded a security event on your account:</p>\n<ul>\n<li><strong>Event:</strong> {event}</li>\n<li><strong>Detail:</strong> {detail}</li>\n</ul>\n<p style=\"font-size:13px;\">If this was you, no action is needed. If it was not, change your password now and review your active sessions and API keys.</p>",
            event = esc(event),
            detail = esc(detail)
        ),
    );
    Rendered {
        template_key: "security_alert",
        subject,
        body_text: text,
        body_html: html,
    }
}

/// Invoice receipt after a successful subscription payment.
pub fn invoice_receipt(
    to: &str,
    organization_name: &str,
    amount_minor: i64,
    currency: &str,
    invoice_id: &str,
    date: &str,
) -> Rendered {
    let _ = to;
    let amount = format_money(amount_minor, currency);
    let subject = format!("Receipt {amount} — {organization_name}");
    let text = format!(
        "Payment received for {organization_name}.\n\nAmount: {amount}\nInvoice: {invoice_id}\nDate: {date}\n\nThis is an automated receipt. Your subscription remains active.",
        organization_name = organization_name,
        amount = amount,
        invoice_id = invoice_id,
        date = date
    );
    let html = wrap(
        "Payment receipt",
        &format!(
            "<h2 style=\"margin-top:0;\">Payment received</h2>\n<table style=\"border-collapse:collapse;font-size:14px;\">\n<tr><td style=\"padding:4px 16px 4px 0;color:#6b7280;\">Organization</td><td>{org}</td></tr>\n<tr><td style=\"padding:4px 16px 4px 0;color:#6b7280;\">Amount</td><td><strong>{amount}</strong></td></tr>\n<tr><td style=\"padding:4px 16px 4px 0;color:#6b7280;\">Invoice</td><td>{invoice}</td></tr>\n<tr><td style=\"padding:4px 16px 4px 0;color:#6b7280;\">Date</td><td>{date}</td></tr>\n</table>\n<p style=\"font-size:13px;\">This is an automated receipt. Your subscription remains active.</p>",
            org = esc(organization_name),
            amount = esc(&amount),
            invoice = esc(invoice_id),
            date = esc(date)
        ),
    );
    Rendered {
        template_key: "invoice_receipt",
        subject,
        body_text: text,
        body_html: html,
    }
}

/// Format minor units (cents) as a display amount. Uses checked math; a
/// negative amount renders with a leading `-` rather than wrapping.
fn format_money(amount_minor: i64, currency: &str) -> String {
    let negative = amount_minor < 0;
    let abs = amount_minor.unsigned_abs();
    let major = abs / 100;
    let minor = abs % 100;
    format!(
        "{}{}.{:02} {}",
        if negative { "-" } else { "" },
        major,
        minor,
        currency.to_ascii_uppercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_the_html_danger_set() {
        let evil = r#"<script>alert("x&y")</script>"#;
        let out = esc(evil);
        assert!(!out.contains('<'));
        assert!(!out.contains('>'));
        assert!(out.contains("&lt;script&gt;"));
        assert!(out.contains("&quot;"));
        assert!(out.contains("&amp;"));
    }

    #[test]
    fn every_template_declares_its_key_and_escapes_inputs() {
        let evil_link = "https://x/?a=<b>&\"'";
        let v = email_verification("u@example.com", evil_link, 30);
        assert_eq!(v.template_key, "email_verification");
        assert!(!v.body_html.contains("<b>"));
        assert!(v.body_text.contains(evil_link), "plain text keeps raw URL");

        let r = password_reset("u@example.com", "https://x/t", 30);
        assert_eq!(r.template_key, "password_reset");
        let i = member_invite("u@example.com", "A<b>", "Org\"x", "https://x/i", "admin");
        assert_eq!(i.template_key, "member_invite");
        assert!(!i.body_html.contains("A<b>"));
        assert!(i.body_html.contains("A&lt;b&gt;"));
        let a = security_alert("u@example.com", "new_login", "Dhaka, BD");
        assert_eq!(a.template_key, "security_alert");
        let inv = invoice_receipt("u@example.com", "Org", 1999, "usd", "in_1", "2026-10-07");
        assert_eq!(inv.template_key, "invoice_receipt");
        assert!(inv.subject.contains("19.99 USD"));
    }

    #[test]
    fn money_formatting_handles_signs_and_zeros() {
        assert_eq!(format_money(0, "usd"), "0.00 USD");
        assert_eq!(format_money(5, "eur"), "0.05 EUR");
        assert_eq!(format_money(-1999, "usd"), "-19.99 USD");
        assert_eq!(format_money(i64::MIN, "usd").starts_with('-'), true);
    }
}
