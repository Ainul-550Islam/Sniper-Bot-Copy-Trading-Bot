#!/bin/bash
source /home/user/.cargo/env 2>/dev/null
export PATH=/home/user/.cargo/bin:$PATH
export RUSTFLAGS=" "
cd /home/user/Sniper-Bot-Copy-Trading-Bot
CARGO_BUILD_JOBS=1 cargo test --locked --no-fail-fast -j 1 -p bot-core -p solana-kit -p module-sniper -p module-copy > /home/user/test.log 2>&1
echo "TEST_EXIT=$?" >> /home/user/test.log
