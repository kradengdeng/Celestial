fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("app_icon.ico"); // Path to your icon
        res.compile().unwrap();
    }
}