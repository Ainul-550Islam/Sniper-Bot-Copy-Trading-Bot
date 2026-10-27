//! `dataset_tool` — std::env::args CLI (no clap) for BATCH 02 datasets.
//!
//! Commands:
//!   verify <dir>                              recompute hash chain + coverage; prints verdict
//!   inspect <dir>                             pretty-print the manifest + verifier summary
//!   import-fixtures <fixtures_dir> <out_dir>  import replay fixtures as a synthetic_fixture dataset
//!   replay-hash <dir>                         deterministic hash of the replay output
//!
//! Exit code is non-zero on ANY failure (bad args, unreadable dataset, a
//! verify whose outcome is `fail`, ...). `verify`/`import-fixtures` print the
//! dataset `source` and `authoritative` so honesty is machine-checkable.

use std::path::Path;

use module_sniper::dataset::fixture_import::import_fixtures;
use module_sniper::dataset::format::DatasetManifest;
use module_sniper::dataset::policy::VerifierPolicy;
use module_sniper::dataset::recorded_source::RecordedReplaySource;
use module_sniper::dataset::verifier::{verify_dir, VerificationOutcome};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(dispatch(&args));
}

fn dispatch(args: &[String]) -> i32 {
    if args.len() < 2 {
        usage();
        return 2;
    }
    let rest = &args[2..];
    let result = match args[1].as_str() {
        "verify" => cmd_verify(rest),
        "inspect" => cmd_inspect(rest),
        "import-fixtures" => cmd_import_fixtures(rest),
        "replay-hash" => cmd_replay_hash(rest),
        other => {
            eprintln!("unknown command: {other}");
            usage();
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn usage() {
    eprintln!(
        "usage: dataset_tool <command> [args]\n\
         \n\
         commands:\n\
         \x20 verify <dir>                              recompute hash chain + coverage\n\
         \x20 inspect <dir>                             show manifest + verifier summary\n\
         \x20 import-fixtures <fixtures_dir> <out_dir>  import replay fixtures (synthetic)\n\
         \x20 replay-hash <dir>                         deterministic output hash"
    );
}

fn need<'a>(args: &'a [String], index: usize, what: &str) -> Result<&'a str, String> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {what}"))
}

fn print_report(manifest: &DatasetManifest, report: &module_sniper::dataset::VerificationReport) {
    println!("dataset_id={}", manifest.dataset_id);
    println!("source={}", manifest.source.as_str());
    println!("status={}", manifest.status.as_str());
    println!("schema_version={}", manifest.schema_version);
    println!("record_count={}", report.record_count);
    println!("chunk_count={}", report.chunk_count);
    println!("sha256_root={}", report.sha256_root);
    println!("verifier_version={}", report.verifier_version);
    println!("hash_chain_ok={}", report.hash_chain_ok);
    println!("coverage_ok={}", report.coverage_ok);
    println!("gaps_found={}", report.gaps_found);
    println!("max_slot_gap={}", report.max_slot_gap);
    println!("outcome={}", report.outcome.as_str());
    println!("authoritative={}", report.authoritative);
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let dir = need(args, 0, "<dir>")?;
    let policy = VerifierPolicy::from_env().map_err(|e| e.to_string())?;
    let (manifest, report) = verify_dir(Path::new(dir), &policy).map_err(|e| e.to_string())?;
    print_report(&manifest, &report);
    if report.outcome != VerificationOutcome::Pass {
        return Err(format!(
            "verification FAILED for {} (hash_chain_ok={} coverage_ok={})",
            manifest.dataset_id, report.hash_chain_ok, report.coverage_ok
        ));
    }
    Ok(())
}

fn cmd_inspect(args: &[String]) -> Result<(), String> {
    let dir = need(args, 0, "<dir>")?;
    let policy = VerifierPolicy::from_env().map_err(|e| e.to_string())?;
    let (manifest, report) = verify_dir(Path::new(dir), &policy).map_err(|e| e.to_string())?;
    let pretty = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    println!("{pretty}");
    println!("--- verifier summary ---");
    print_report(&manifest, &report);
    Ok(())
}

fn cmd_import_fixtures(args: &[String]) -> Result<(), String> {
    let fixtures = need(args, 0, "<fixtures_dir>")?;
    let out = need(args, 1, "<out_dir>")?;
    let manifest =
        import_fixtures(Path::new(fixtures), Path::new(out)).map_err(|e| e.to_string())?;
    // Machine-readable pointer to the just-created dataset directory.
    std::fs::write(
        Path::new(out).join("LATEST"),
        manifest.dataset_id.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "imported {} records across {} chunk(s) into {}/{}",
        manifest.record_count,
        manifest.chunks.len(),
        out,
        manifest.dataset_id
    );
    println!("dataset_id={}", manifest.dataset_id);
    println!("source={}", manifest.source.as_str());
    println!("authoritative={}", manifest.authoritative);
    // Bare id on the last line for easy shell capture.
    println!("{}", manifest.dataset_id);
    Ok(())
}

fn cmd_replay_hash(args: &[String]) -> Result<(), String> {
    let dir = need(args, 0, "<dir>")?;
    let policy = VerifierPolicy::from_env().map_err(|e| e.to_string())?;
    let source = RecordedReplaySource::open(Path::new(dir), &policy).map_err(|e| e.to_string())?;
    let output = source.output();
    let hash = output.output_hash().map_err(|e| e.to_string())?;
    println!("{hash}");
    Ok(())
}
