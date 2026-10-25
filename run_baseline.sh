#!/bin/bash
. /home/user/.cargo/env
cd /home/user/baseline
export CARGO_TARGET_DIR=/home/user/sniper-suite/target
cargo test -p module-sniper --test failure_injection --test gates_risk_intel --test pipeline --no-fail-fast -- --test-threads=1 > /home/user/baseline-sniper.log 2>&1
echo "EXIT=$?" >> /home/user/baseline-sniper.log
cargo test -p sniper-suite --test batch6_transaction_readiness --test custody_rotation_cross_tenant --no-fail-fast -- --test-threads=1 > /home/user/baseline-server.log 2>&1
echo "EXIT=$?" >> /home/user/baseline-server.log
