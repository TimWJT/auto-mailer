fn main() {
    // config.rs reads this at compile time; rebuild when it changes.
    println!("cargo:rerun-if-env-changed=AUTO_MAILER_MS_CLIENT_ID");
    tauri_build::build()
}
