#!/usr/bin/env bash
# helm_image_contract.sh — regression test for the Helm immutable-image contract.
#
# Static checks always run. When the `helm` binary is present the test ALSO
# renders the chart — lint + template with a digest, and template with the empty
# default to prove the chart fails closed. Where helm is absent those two steps
# report NOT_RUN instead of pretending to pass.
#   1. values.yaml has an empty image.digest and no mutable image.tag key;
#   2. the sniper-suite.image helper enforces sha256:<64 lowercase hex> and
#      fails the render when the digest is missing or malformed;
#   3. deployment and migration Job both render the image via that helper;
#   4. no template builds an image from .Values.image.tag.
# Exit non-zero on the first violated assertion.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHART="$ROOT/deploy/helm/sniper-suite"
VALUES="$CHART/values.yaml"
HELPERS="$CHART/templates/_helpers.tpl"
fail() { echo "[helm-image-contract] FAIL — $*" >&2; exit 1; }

grep -qE '^  digest: ""$' "$VALUES" || fail "values.yaml must declare image.digest with an empty default"
if grep -qE '^  tag:' "$VALUES"; then fail "values.yaml still declares a mutable image.tag"; fi

grep -qF 'define "sniper-suite.image"' "$HELPERS" || fail "helper sniper-suite.image is missing"
grep -qF 'required "image.digest is required' "$HELPERS" || fail "helper does not require image.digest"
grep -qF 'regexMatch "^sha256:[a-f0-9]{64}$"' "$HELPERS" || fail "helper does not validate the sha256 digest form"

for t in deployment.yaml migration-job.yaml; do
  grep -qF 'include "sniper-suite.image" . | quote' "$CHART/templates/$t" \
    || fail "$t does not render the image through sniper-suite.image"
done

if grep -rn '\.Values\.image\.tag' "$CHART/templates" >/dev/null 2>&1; then
  fail "a template still builds an image from .Values.image.tag"
fi

if command -v helm >/dev/null 2>&1; then
    # A syntactically valid digest that is NOT a published image: it only
    # exercises the render path, and is never referenced as a real image.
    TEST_DIGEST="sha256:$(printf 'sniper-suite-chart-render-test' | sha256sum | cut -d' ' -f1)"
    helm lint "$CHART" --set image.digest="$TEST_DIGEST" >/dev/null \
        || fail "helm lint failed with a valid digest"
    rendered="$(helm template sniper "$CHART" --set image.digest="$TEST_DIGEST" --set migrationJob.enabled=true)"
    grep -qF "image: \"sniper-suite@$TEST_DIGEST\"" <<<"$rendered" || fail "deployment did not render the digest-pinned image"
    [ "$(grep -cF "sniper-suite@$TEST_DIGEST" <<<"$rendered")" -ge 2 ] \
        || fail "migration Job did not render the digest-pinned image"
    if helm template sniper "$CHART" >/dev/null 2>&1; then
        fail "chart rendered with an EMPTY image.digest — the fail-closed contract is broken"
    fi
    echo "[helm-image-contract] PASS — digest-only image contract holds (helm $(helm version --short 2>/dev/null): lint PASS, template PASS, empty-digest render REFUSED)"
else
    echo "[helm-image-contract] PASS — digest-only image contract holds (helm template: NOT_RUN, helm not installed)"
fi
