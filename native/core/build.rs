fn main() {
    // include_dir! embeds the UI at compile time. Watch the asset directory too,
    // so an incremental native build cannot silently retain an older theme.
    println!("cargo:rerun-if-changed=../../dist/ui");
}
