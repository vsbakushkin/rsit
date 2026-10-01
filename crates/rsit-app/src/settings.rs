//! User settings — color theme and font sizes — kept in
//! `settings.json` in [`rsit_git::dirs::config_dir`], and the Settings window (Ctrl+Alt+S).

use std::path::PathBuf;

use gpui_kit::component::setting::{
    NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeRegistry};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

actions!(settings, [OpenSettings, CloseSettings]);

/// Color themes shipped with gpui-component, besides its Default Light/Dark.
const THEMES: &[&str] = &[
    include_str!("../themes/adventure.json"),
    include_str!("../themes/alduin.json"),
    include_str!("../themes/asciinema.json"),
    include_str!("../themes/aurora.json"),
    include_str!("../themes/ayu.json"),
    include_str!("../themes/catppuccin.json"),
    include_str!("../themes/everforest.json"),
    include_str!("../themes/fahrenheit.json"),
    include_str!("../themes/flexoki.json"),
    include_str!("../themes/gruvbox.json"),
    include_str!("../themes/harper.json"),
    include_str!("../themes/hybrid.json"),
    include_str!("../themes/jellybeans.json"),
    include_str!("../themes/kibble.json"),
    include_str!("../themes/macos-classic.json"),
    include_str!("../themes/mellifluous.json"),
    include_str!("../themes/molokai.json"),
    include_str!("../themes/solarized.json"),
    include_str!("../themes/spaceduck.json"),
    include_str!("../themes/tokyonight.json"),
    include_str!("../themes/twilight.json"),
];

pub const DEFAULT_THEME: &str = "Default Light";
/// gpui-component's base size: the rem that `text_sm` and friends scale from.
pub const DEFAULT_FONT_SIZE: f32 = 16.0;
/// Size of code in diff, merge and annotate views.
pub const DEFAULT_EDITOR_FONT_SIZE: f32 = 13.0;
const FONT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=32.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: String,
    pub font_size: f32,
    pub editor_font_size: f32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { theme: DEFAULT_THEME.into(), font_size: DEFAULT_FONT_SIZE, editor_font_size: DEFAULT_EDITOR_FONT_SIZE }
    }
}

impl Global for AppSettings {}

pub fn init(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for themes in THEMES {
        if let Err(e) = registry.load_themes_from_str(themes) {
            eprintln!("rsit: bad bundled theme: {e:#}");
        }
    }
    cx.set_global(AppSettings::default());
    cx.bind_keys([
        KeyBinding::new("ctrl-alt-s", OpenSettings, None),
        KeyBinding::new("escape", CloseSettings, Some("SettingsWindow")),
    ]);
    cx.on_action(|_: &OpenSettings, cx| open(cx));
}

/// Reads the settings file and applies it. Not called by tests, which keep the defaults.
pub fn load(cx: &mut App) {
    let Some(path) = settings_path() else { return };
    cx.set_global(SettingsFile(path.clone()));
    let settings = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<AppSettings>(&text) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rsit: {}: {e}", path.display());
                return;
            }
        },
        Err(_) => return,
    };
    cx.set_global(settings);
    apply(cx);
}

/// Edits the settings, applies them to every window and saves them.
pub fn update(cx: &mut App, edit: impl FnOnce(&mut AppSettings)) {
    let mut settings = cx.global::<AppSettings>().clone();
    edit(&mut settings);
    settings.font_size = settings.font_size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
    settings.editor_font_size = settings.editor_font_size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
    if settings == *cx.global::<AppSettings>() {
        return;
    }
    cx.set_global(settings);
    apply(cx);
    save(cx);
}

fn apply(cx: &mut App) {
    let settings = cx.global::<AppSettings>().clone();
    let registry = ThemeRegistry::global(cx);
    let config =
        registry.themes().get(settings.theme.as_str()).unwrap_or_else(|| registry.default_light_theme()).clone();
    // switching mode may reload the mode's theme after the closure, so set the
    // fonts in a second update
    Theme::update(cx, |theme| theme.apply_config(&config));
    Theme::update(cx, |theme| {
        theme.font_size = px(settings.font_size);
        theme.mono_font_size = px(settings.editor_font_size);
    });
}

/// Where settings are saved; absent until [`load`], so tests never write.
struct SettingsFile(PathBuf);

impl Global for SettingsFile {}

fn save(cx: &App) {
    let Some(SettingsFile(path)) = cx.try_global::<SettingsFile>() else { return };
    let text = serde_json::to_string_pretty(cx.global::<AppSettings>()).expect("settings serialize");
    let result = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(path, text));
    if let Err(e) = result {
        eprintln!("rsit: cannot save {}: {e}", path.display());
    }
}

fn settings_path() -> Option<PathBuf> {
    Some(rsit_git::dirs::config_dir()?.join("settings.json"))
}

/// Scale of list rows: they follow the UI font size.
pub fn ui_scale(cx: &App) -> f32 {
    f32::from(cx.theme().font_size) / DEFAULT_FONT_SIZE
}

/// Scale of code rows: they follow the editor font size.
pub fn editor_scale(cx: &App) -> f32 {
    f32::from(cx.theme().mono_font_size) / DEFAULT_EDITOR_FONT_SIZE
}

/// A row height designed for the default font, scaled by `scale` and kept whole.
pub fn scaled(height: f32, scale: f32) -> Pixels {
    px((height * scale).round())
}

/// Calls `on_change` when the syntax theme changes, to recolor highlighted text.
pub fn observe_highlight_theme<V: 'static>(
    cx: &mut Context<V>,
    on_change: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> Subscription {
    let mut current = cx.theme().highlight_theme.clone();
    cx.observe_global::<Theme>(move |this, cx| {
        let theme = cx.theme().highlight_theme.clone();
        if !std::sync::Arc::ptr_eq(&theme, &current) {
            current = theme;
            on_change(this, cx);
        }
    })
}

struct SettingsWindow {
    focus: FocusHandle,
}

/// The open Settings window, if any.
struct SettingsWindowHandle(AnyWindowHandle);

impl Global for SettingsWindowHandle {}

/// Opens the Settings window, or brings the open one to front.
pub fn open(cx: &mut App) {
    if let Some(SettingsWindowHandle(handle)) =
        cx.try_global::<SettingsWindowHandle>().map(|h| SettingsWindowHandle(h.0))
        && handle.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions { title: Some("Settings — rsit".into()), ..Default::default() }),
        window_bounds: Some(WindowBounds::centered(size(px(760.), px(480.)), cx)),
        app_id: Some("rsit".into()),
        ..Default::default()
    };
    let result = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            SettingsWindow { focus }
        })
    });
    match result {
        Ok((handle, _)) => cx.set_global(SettingsWindowHandle(handle)),
        Err(e) => eprintln!("rsit: cannot open settings: {e:#}"),
    }
}

fn appearance_page(cx: &App) -> SettingPage {
    let themes: Vec<(SharedString, SharedString)> =
        ThemeRegistry::global(cx).sorted_themes().into_iter().map(|t| (t.name.clone(), t.name.clone())).collect();
    let sizes = NumberFieldOptions { min: *FONT_SIZES.start() as f64, max: *FONT_SIZES.end() as f64, step: 1.0 };
    SettingPage::new("Appearance").default_open(true).group(
        SettingGroup::new()
            .item(SettingItem::new(
                "Theme",
                SettingField::scrollable_dropdown(
                    themes,
                    |cx| cx.global::<AppSettings>().theme.clone().into(),
                    |name, cx| update(cx, |s| s.theme = name.to_string()),
                )
                .default_value(DEFAULT_THEME),
            ))
            .item(
                SettingItem::new(
                    "Font size",
                    SettingField::number_input(
                        sizes.clone(),
                        |cx| cx.global::<AppSettings>().font_size as f64,
                        |size, cx| update(cx, |s| s.font_size = size as f32),
                    )
                    .default_value(DEFAULT_FONT_SIZE as f64),
                )
                .description("Size of the interface text; list rows grow with it."),
            )
            .item(
                SettingItem::new(
                    "Editor font size",
                    SettingField::number_input(
                        sizes,
                        |cx| cx.global::<AppSettings>().editor_font_size as f64,
                        |size, cx| update(cx, |s| s.editor_font_size = size as f32),
                    )
                    .default_value(DEFAULT_EDITOR_FONT_SIZE as f64),
                )
                .description("Size of code in the diff, merge and annotate views."),
            ),
    )
}

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-window")
            .key_context("SettingsWindow")
            .track_focus(&self.focus)
            .on_action(|_: &CloseSettings, window, _| window.remove_window())
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(Settings::new("settings").sidebar_width(px(180.)).page(appearance_page(cx)))
    }
}
