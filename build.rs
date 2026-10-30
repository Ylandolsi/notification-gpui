//! Version info (and the app icon, once `assets/notchd.ico` exists) for notchd.exe.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "notchd");
    res.set("FileDescription", "notchd notification centre");
    if std::path::Path::new("assets/notchd.ico").exists() {
        res.set_icon("assets/notchd.ico");
    }
    println!("cargo:rerun-if-changed=assets/notchd.ico");
    if let Err(err) = res.compile() {
        println!("cargo:warning=could not embed Windows resources: {err}");
    }
}
