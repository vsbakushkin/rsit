fn main() {
    // the icon Explorer, the taskbar and Alt+Tab show for rsit.exe
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("rsit.rc", embed_resource::NONE).manifest_required().unwrap();
    }
}
