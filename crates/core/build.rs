fn main() {
    // sqlx::migrate! embeds the migration directory. New migration files must
    // invalidate cached builds too, not just edits to already embedded files.
    println!("cargo:rerun-if-changed=migrations");
}
