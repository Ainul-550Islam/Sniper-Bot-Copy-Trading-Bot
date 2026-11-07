# Email delivery and invitation security

Invitation creation and resend persist the invitation hash and the recipient
mail in one PostgreSQL transaction. The plaintext bearer token is placed only
in the recipient's rendered email/outbox message; it is never returned to the
inviter or included in the invitation list. A recipient's ability to use the
link is the email-ownership proof. Invitation acceptance must not be exposed
through an administrative token-return endpoint.

Email delivery is **off by default**. When disabled, queued messages remain in
`email_outbox`; the API reports `queued_for_delivery`, which means persisted in
the outbox, not confirmed delivered. Configure and monitor a provider before
relying on invitations, password-reset links, or verification links.

Enable delivery explicitly with `EMAIL_ENABLED=true` and `EMAIL_FROM`. Choose
`EMAIL_BACKEND=resend` (default), `postmark`, `ses`, or `smtp`. Secret values
remain in environment variables; the `*_ENV` settings below contain variable
names, not secret values:

- Resend: `EMAIL_RESEND_API_KEY_ENV` (default `RESEND_API_KEY`).
- Postmark: `EMAIL_POSTMARK_TOKEN_ENV` (default `POSTMARK_SERVER_TOKEN`).
- SES: `EMAIL_SES_REGION`, `EMAIL_SES_ACCESS_KEY_ENV` (default
  `AWS_ACCESS_KEY_ID`), and `EMAIL_SES_SECRET_KEY_ENV` (default
  `AWS_SECRET_ACCESS_KEY`).
- SMTP: `EMAIL_SMTP_HOST`, `EMAIL_SMTP_PORT` (default 587),
  `EMAIL_SMTP_IMPLICIT_TLS` (default false; STARTTLS is mandatory when false),
  `EMAIL_SMTP_USERNAME`, `EMAIL_SMTP_PASSWORD_ENV` (default
  `SMTP_PASSWORD`), and optional `EMAIL_SMTP_CA_BUNDLE`.
- Optional send ceiling: `EMAIL_PER_MINUTE_LIMIT` (default 0, dispatcher
  conservative default). `PUBLIC_BASE_URL` sets the origin used for recipient
  links; without it, links are same-origin relative paths.

An enabled provider is constructed during startup. Invalid provider settings
fail startup rather than silently leaving the dispatcher inactive. A database
must also be attached for the dispatcher and outbox to operate. Provider send
success means accepted by the provider; it is not proof of inbox delivery or
recipient identity beyond possession of the mailbox link.
