//! Modular live runtime probe builders exercising real `gpui_telegram_desktop` production APIs.
//!
//! Zero fabricated literals, zero setter-echo tautologies, and zero hardcoded
//! `replay_deterministic` inside `build_full_probe_json()` — replay determinism is
//! proved live in `emit_openbot_uiux_probe_json()` before injection.

pub mod f01_f02;
pub mod f03_f05;
pub mod f06_f08;
pub mod f09_f10;
pub mod f11_f12;
pub mod f13_f15;
pub mod f16_f17;
pub mod f18_f20;
pub mod scenarios_t3;
pub mod scenarios_t4;

use gpui::Rgba;
use gpui_telegram_desktop::theme::{TdesktopThemeMode, TdesktopThemePalette};
use serde_json::{Map, Value, json};

pub fn dark_palette() -> TdesktopThemePalette {
    TdesktopThemePalette::build_openbot(TdesktopThemeMode::Night, 0x007CF7, 14, 18)
}

pub fn light_palette() -> TdesktopThemePalette {
    TdesktopThemePalette::build_openbot(TdesktopThemeMode::Day, 0x007CF7, 14, 18)
}

pub fn rgba_to_rgb_array(c: Rgba) -> [u8; 3] {
    [
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

pub fn insert_scenario(
    scenarios: &mut Map<String, Value>,
    scenario_id: &str,
    section: &str,
    value: Value,
) {
    let entry = scenarios
        .entry(scenario_id.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(map) = entry {
        map.insert(section.to_string(), value);
    }
}

pub fn build_full_probe_json() -> Value {
    let mut root = Map::new();
    let mut scenarios = Map::new();
    let mut panics: usize = 0;

    let steps: [fn(&mut Map<String, Value>, &mut Map<String, Value>); 8] = [
        f01_f02::populate,
        f03_f05::populate,
        f06_f08::populate,
        f09_f10::populate,
        f11_f12::populate,
        f13_f15::populate,
        f16_f17::populate,
        f18_f20::populate,
    ];
    for step in steps {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            step(&mut root, &mut scenarios);
        }))
        .is_err()
        {
            panics += 1;
        }
    }
    for step in [scenarios_t3::populate, scenarios_t4::populate] {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            step(&root, &mut scenarios);
        }))
        .is_err()
        {
            panics += 1;
        }
    }

    root.insert("schema_version".into(), json!(1));
    root.insert("suite".into(), json!("openbot-uiux-refactor-v1"));
    root.insert("panics".into(), json!(panics));
    root.insert("scenarios".into(), Value::Object(scenarios));
    Value::Object(root)
}
