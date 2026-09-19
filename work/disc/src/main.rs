use sha2::{Digest, Sha256};
fn sighash(prefix: &str, name: &str) -> [u8; 8] {
    let mut h = Sha256::new();
    h.update(format!("{prefix}:{name}").as_bytes());
    let d = h.finalize();
    let mut o = [0u8; 8];
    o.copy_from_slice(&d[..8]);
    o
}
fn show(label: &str, v: [u8; 8]) {
    println!("{label:36} {:?}  0x{}", v, hex::encode(v));
}
fn main() {
    for n in ["buy","sell","create","create_v2","buy_exact_sol_in","buy_v2","sell_v2","migrate","initialize","extend_account"] {
        show(&format!("global:{n}"), sighash("global", n));
    }
    for n in ["Global","BondingCurve","FeeConfig","GlobalVolumeAccumulator","UserVolumeAccumulator","Pool"] {
        show(&format!("account:{n}"), sighash("account", n));
    }
}
