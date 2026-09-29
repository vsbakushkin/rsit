use gpui_kit::AssetSource as _;
use gpui_kit::assets::IconName;

#[test]
fn extra_icons_load() {
    for icon in [IconName::GitBranch, IconName::ArrowDownToLine, IconName::ArrowUpFromLine, IconName::RefreshCw] {
        let path = icon.path();
        let data = rsit_app::AppAssets.load(&path).unwrap();
        assert!(data.is_some(), "missing {path}");
    }
}
