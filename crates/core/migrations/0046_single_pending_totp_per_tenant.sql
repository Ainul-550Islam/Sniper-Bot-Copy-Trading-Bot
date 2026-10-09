-- A single unverified TOTP enrollment may be pending for each user and
-- tenant. Concurrent setup requests otherwise create multiple live secrets;
-- the user can verify a secret that a later setup request has already
-- replaced. Keep the newest pending enrollment and discard only older,
-- unverified setup rows before enforcing the invariant.
WITH ranked_pending AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY user_id, organization_id, device_type
               ORDER BY created_at DESC, id DESC
           ) AS row_number
      FROM user_mfa_devices
     WHERE device_type = 'totp'
       AND verified = false
)
DELETE FROM user_mfa_devices AS device
 USING ranked_pending AS pending
 WHERE device.id = pending.id
   AND pending.row_number > 1;

CREATE UNIQUE INDEX IF NOT EXISTS user_mfa_one_pending_totp_per_tenant
    ON user_mfa_devices (user_id, organization_id)
    WHERE device_type = 'totp' AND verified = false;
