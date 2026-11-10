// Tanam ikon dan manifest (Common Controls v6, DPI per monitor) ke exe.
// Hanya untuk toolchain MSVC, yang dipakai build GitHub Actions; rc.exe ada di Windows SDK.
fn main() {
    println!("cargo:rerun-if-changed=resources.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=NotchMusic.ico");
    println!("cargo:rerun-if-changed=NotchMusic_Tray_Mint.ico");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        embed_resource::compile("resources.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
}
