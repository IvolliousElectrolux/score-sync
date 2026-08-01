fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/app.ico");
        println!("cargo:rerun-if-changed=assets/app.rc");
        embed_resource::compile("assets/app.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
