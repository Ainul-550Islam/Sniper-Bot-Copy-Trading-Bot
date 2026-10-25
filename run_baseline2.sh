#!/bin/bash
. /home/user/.cargo/env
cd /home/user/baseline
export CARGO_TARGET_DIR=/home/user/sniper-suite/target
cargo test -p sniper-suite --test custody_rotation_missing_profile --test fresh_tenant_contract --test invite_flow --test mfa_flow --test release_manifest_integration --test tenant_stream_isolation --no-fail-fast -- --test-threads=1 > /home/user/baseline-server2.log 2>&1
echo "EXIT=$?" >> /home/user/baseline-server2.log
cargo test -p solana-kit --test latency_bench --no-fail-fast -- --test-threads=1 > /home/user/baseline-latency.log 2>&1
echo "EXIT=$?" >> /home/user/baseline-latency.log
