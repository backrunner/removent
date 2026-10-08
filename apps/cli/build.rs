fn main() {
    // rust-i18n embeds these files in its procedural macro. Cargo otherwise
    // reuses the binary when only a translation changes.
    println!("cargo:rerun-if-changed=locales");
}
