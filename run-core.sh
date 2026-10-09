#!/bin/bash
export PATH=/home/user/.cargo/bin:$PATH
export RUSTFLAGS=" "
cd /home/user/Sniper-Bot-Copy-Trading-Bot
cargo test --locked -p bot-core --lib execution_reliability_defaults > /home/user/core-test.log 2>&1
echo "EXIT=$?" >> /home/user/core-test.log
