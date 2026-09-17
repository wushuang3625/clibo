fn main() {
    println!("cargo:rerun-if-changed=icons/clibo.rc");
    println!("cargo:rerun-if-changed=icons/clibo.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for("icons/clibo.rc", ["clibo-native"], embed_resource::NONE)
            .manifest_required()
            .expect("Failed to embed the Clibo application icon");
    }
}
