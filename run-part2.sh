#!/bin/bash
export PATH=/home/user/.cargo/bin:$PATH
export RUSTFLAGS=" "
cd /home/user/Sniper-Bot-Copy-Trading-Bot
cargo test --locked --no-fail-fast -p solana-kit -p module-sniper -p module-copy -p bot-core --all-targets > /home/user/part2-test.log 2>&1
echo "EXIT=$?" >> /home/user/part2-test.log
