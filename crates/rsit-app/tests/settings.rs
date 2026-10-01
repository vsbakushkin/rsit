//! Themes and font sizes from the Settings window.

use gpui_kit::TestAppContext;
use gpui_kit::component::{ActiveTheme as _, ThemeRegistry};
use gpui_kit::px;
use rsit_app::settings::{self, AppSettings};

#[gpui_kit::test]
fn bundled_themes_load(cx: &mut TestAppContext) {
    cx.update(|cx| {
        rsit_app::init(cx);
        let themes = ThemeRegistry::global(cx).themes();
        for name in ["Default Light", "Default Dark", "Tokyo Night", "Tokyo Storm", "Catppuccin Mocha", "Gruvbox Dark"]
        {
            assert!(themes.contains_key(name), "missing theme {name}");
        }
        assert!(themes.len() >= 38, "only {} themes", themes.len());
    });
}

#[gpui_kit::test]
fn update_applies_theme_and_font_sizes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        rsit_app::init(cx);
        assert!(!cx.theme().is_dark());
        settings::update(cx, |s| {
            s.theme = "Tokyo Night".into();
            s.font_size = 18.0;
            s.editor_font_size = 100.0;
        });
        let theme = cx.theme();
        assert!(theme.is_dark());
        assert_eq!(theme.theme_name().as_ref(), "Tokyo Night");
        assert_eq!(theme.font_size, px(18.));
        // clamped to the allowed range
        assert_eq!(theme.mono_font_size, px(32.));
        assert_eq!(settings::ui_scale(cx), 18.0 / 16.0);

        // back to light keeps the chosen sizes
        settings::update(cx, |s| s.theme = "Default Light".into());
        assert!(!cx.theme().is_dark());
        assert_eq!(cx.theme().font_size, px(18.));
        assert_eq!(cx.global::<AppSettings>().editor_font_size, 32.0);
    });
}
