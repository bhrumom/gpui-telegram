use gpui::prelude::FluentBuilder;
use gpui::*;
use crate::i18n::tr_str;
use crate::theme::assets::IconName;
use crate::theme::palette::{TdesktopThemeMode, TdesktopThemePalette};
use crate::views::settings::settings_theme_ui::{
    settings_action_row, settings_container, settings_group_card, settings_section_block,
    settings_static_row, settings_toggle_row,
};

pub const K_MIC_TEST_UPDATE_INTERVAL_MS: u64 = 100;
pub const K_MIC_TEST_ANIMATION_DURATION_MS: u64 = 200;

pub struct SettingsCallsSections {
    pub input_device: String,
    pub output_device: String,
    pub camera_device: String,
    pub available_inputs: Vec<String>,
    pub available_outputs: Vec<String>,
    pub available_cameras: Vec<String>,
    pub mic_level: f32,
    pub mic_test_active: bool,
    pub p2p_mode: String,
    pub accept_calls: bool,
    pub pending_save_settings: bool,
    pub pending_prefs_sync: bool,
    pub pending_mic_test_change: Option<bool>,
    pub is_visible: bool,
    pub palette: TdesktopThemePalette,
    pub language_id: String,
}

impl SettingsCallsSections {
    pub fn new() -> Self {
        Self {
            input_device: "Default Microphone".into(),
            output_device: "Default Output Device".into(),
            camera_device: "Default Camera".into(),
            available_inputs: vec!["Default Microphone".into(), "Built-in Mic".into()],
            available_outputs: vec!["Default Output Device".into(), "Speakers".into()],
            available_cameras: vec!["Default Camera".into(), "FaceTime HD Camera".into()],
            mic_level: 0.0,
            mic_test_active: false,
            p2p_mode: "Contacts".into(),
            accept_calls: true,
            pending_save_settings: false,
            pending_prefs_sync: false,
            pending_mic_test_change: None,
            is_visible: false,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3390ec, 14, 12),
            language_id: "en".into(),
        }
    }

    pub fn new_live(palette: TdesktopThemePalette, language_id: &str) -> Self {
        let mut s = Self::new();
        s.input_device = String::new();
        s.output_device = String::new();
        s.camera_device = String::new();
        s.available_inputs.clear();
        s.available_outputs.clear();
        s.available_cameras.clear();
        s.palette = palette;
        s.language_id = language_id.into();
        s.is_visible = true;
        s
    }

    pub fn sync_devices(&mut self, inputs: Vec<String>, outputs: Vec<String>, cameras: Vec<String>) {
        if !inputs.is_empty() {
            if !inputs.contains(&self.input_device) {
                self.input_device = inputs[0].clone();
            }
            self.available_inputs = inputs;
        }
        if !outputs.is_empty() {
            if !outputs.contains(&self.output_device) {
                self.output_device = outputs[0].clone();
            }
            self.available_outputs = outputs;
        }
        if !cameras.is_empty() {
            if !cameras.contains(&self.camera_device) {
                self.camera_device = cameras[0].clone();
            }
            self.available_cameras = cameras;
        }
    }

    pub fn set_palette(&mut self, palette: TdesktopThemePalette) { self.palette = palette; }
    pub fn set_language(&mut self, language_id: impl Into<String>) { self.language_id = language_id.into(); }
    pub fn calls_id(&self) -> &'static str { "settings_calls" }
    pub fn toggle_visibility(&mut self) { self.is_visible = !self.is_visible; }

    pub fn calculate_normalized_level(&self, rms: f32, min_rms: f32, max_rms: f32) -> f32 {
        if max_rms <= min_rms { return 0.0; }
        ((rms - min_rms) / (max_rms - min_rms)).clamp(0.0, 1.0)
    }

    pub fn toggle_mic_test(&mut self) {
        self.mic_test_active = !self.mic_test_active;
        if !self.mic_test_active {
            self.mic_level = 0.0;
        }
        self.pending_mic_test_change = Some(self.mic_test_active);
    }

    pub fn set_mic_level_normalized(&mut self, level: f32) {
        self.mic_level = level.clamp(0.0, 1.0);
    }

    pub fn stop_mic_test_silent(&mut self) {
        self.mic_test_active = false;
        self.mic_level = 0.0;
        self.pending_mic_test_change = None;
    }

    pub fn update_mic_level(&mut self, rms: f32) { self.mic_level = self.calculate_normalized_level(rms, -60.0, 0.0); }
    pub fn set_p2p_mode(&mut self, mode: &str) { self.p2p_mode = mode.into(); self.pending_save_settings = true; }
    pub fn choose_playback_device(&mut self, device: &str) { self.output_device = device.into(); }
    pub fn choose_capture_device(&mut self, device: &str) { self.input_device = device.into(); }
    pub fn choose_camera_device(&mut self, device: &str) { self.camera_device = device.into(); }
    pub fn lng_group_call_speakers(&self) -> &'static str { "Speakers" }
    pub fn lng_settings_call_accept_calls(&self) -> bool { self.accept_calls }
    pub fn lng_settings_call_section_output(&self) -> &'static str { "Speakers and Headphones" }
    pub fn lng_settings_call_output_device(&self) -> &str { &self.output_device }
    pub fn lng_settings_call_camera(&self) -> &str { &self.camera_device }
    pub fn playback_device_name_value(&self) -> String { self.output_device.clone() }
    pub fn capture_device_name_value(&self) -> String { self.input_device.clone() }
    pub fn camera_device_name_value(&self) -> String { self.camera_device.clone() }

    fn cycle(list: &[String], current: &str) -> String {
        let idx = list.iter().position(|x| x == current).map(|i| (i + 1) % list.len()).unwrap_or(0);
        list.get(idx).cloned().unwrap_or_else(|| current.to_string())
    }
}

impl Default for SettingsCallsSections {
    fn default() -> Self { Self::new() }
}

impl Render for SettingsCallsSections {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let ob = pal.openbot_palette();
        let lang = self.language_id.clone();
        let hover_ctrl = ob.bg_control_hover();
        let mic_pct = (self.mic_level * 120.0).clamp(0.0, 120.0);
        let input_display = if self.input_device.is_empty() {
            tr_str(&lang, "系统默认", "System Default").to_string()
        } else {
            self.input_device.clone()
        };
        let output_display = if self.output_device.is_empty() {
            tr_str(&lang, "系统默认", "System Default").to_string()
        } else {
            self.output_device.clone()
        };
        let camera_display = if self.camera_device.is_empty() {
            tr_str(&lang, "系统默认", "System Default").to_string()
        } else {
            self.camera_device.clone()
        };

        let mic_test_controls = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(120.0))
                    .h(px(6.0))
                    .rounded_full()
                    .bg(ob.bg_control_active())
                    .overflow_hidden()
                    .child(div().w(px(mic_pct)).h_full().rounded_full().bg(ob.surface_light())),
            )
            .child(
                div()
                    .id("settings-calls-mic-test-btn")
                    .cursor_pointer()
                    .h(px(28.0))
                    .px(px(11.0))
                    .rounded(px(7.0))
                    .bg(if self.mic_test_active { ob.danger() } else { ob.bg_control() })
                    .border_1()
                    .border_color(if self.mic_test_active { ob.danger() } else { ob.border_strong_hairline() })
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if self.mic_test_active { rgb(0xFFFFFF) } else { ob.text_primary() })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_mic_test();
                            cx.notify();
                        }),
                    )
                    .child(if self.mic_test_active {
                        tr_str(&lang, "停止测试", "Stop Test")
                    } else {
                        tr_str(&lang, "测试麦克风", "Test Mic")
                    }),
            );

        let p2p_segmented = div()
            .p(px(2.0))
            .rounded(px(8.0))
            .bg(ob.bg_control())
            .border_1()
            .border_color(ob.border_hairline())
            .flex()
            .items_center()
            .gap(px(2.0))
            .children(["Everybody", "Contacts", "Nobody"].into_iter().map(|mode| {
                let active = self.p2p_mode == mode;
                let mode_lbl = match mode {
                    "Everybody" => tr_str(&lang, "所有人", "Everyone"),
                    "Contacts" => tr_str(&lang, "联系人", "Contacts"),
                    _ => tr_str(&lang, "禁用", "Nobody"),
                };
                div()
                    .cursor_pointer()
                    .h(px(26.0))
                    .px(px(10.0))
                    .rounded(px(6.0))
                    .when(active, |el| el.bg(ob.surface_light()))
                    .when(!active, |el| el.hover(move |s| s.bg(hover_ctrl)))
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .font_weight(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL })
                    .text_color(if active { ob.text_on_light() } else { ob.text_secondary() })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_p2p_mode(mode);
                            cx.notify();
                        }),
                    )
                    .child(mode_lbl)
            }));

        settings_container(pal)
            .child(settings_section_block(
                tr_str(&lang, "音频设备", "Audio Devices"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![
                        settings_action_row(
                            IconName::Mic,
                            tr_str(&lang, "麦克风", "Microphone"),
                            &input_display,
                            pal,
                            false,
                            |this| {
                                this.input_device = Self::cycle(&this.available_inputs, &this.input_device);
                                this.pending_prefs_sync = true;
                            },
                            cx,
                        ),
                        settings_action_row(
                            IconName::Speaker,
                            tr_str(&lang, "扬声器与耳机", self.lng_settings_call_section_output()),
                            &output_display,
                            pal,
                            false,
                            |this| {
                                this.output_device = Self::cycle(&this.available_outputs, &this.output_device);
                                this.pending_prefs_sync = true;
                            },
                            cx,
                        ),
                        settings_static_row(
                            tr_str(&lang, "麦克风音量检测", "Microphone check"),
                            Some(tr_str(&lang, "实时查看输入电平以确认收音正常。", "Monitor live input level to verify your microphone.")),
                            mic_test_controls,
                            pal,
                        ),
                    ],
                ),
            ))
            .child(settings_section_block(
                tr_str(&lang, "视频与通话连接", "Video & Calling"),
                None,
                pal,
                settings_group_card(
                    pal,
                    vec![
                        settings_action_row(
                            IconName::VideoCall,
                            tr_str(&lang, "摄像头", "Camera"),
                            &camera_display,
                            pal,
                            false,
                            |this| {
                                this.camera_device = Self::cycle(&this.available_cameras, &this.camera_device);
                                this.pending_prefs_sync = true;
                            },
                            cx,
                        ),
                        settings_static_row(
                            tr_str(&lang, "点对点直连 (P2P)", "Peer-to-Peer"),
                            Some(tr_str(&lang, "选择允许建立点对点低延迟语音连接的对象。", "Choose who can connect peer-to-peer for lower latency.")),
                            p2p_segmented,
                            pal,
                        ),
                        settings_toggle_row(
                            IconName::PhoneCall,
                            tr_str(&lang, "允许在此设备接听通话", "Accept calls on this device"),
                            None,
                            self.accept_calls,
                            pal,
                            |this| {
                                this.accept_calls = !this.accept_calls;
                                this.pending_save_settings = true;
                            },
                            cx,
                        ),
                    ],
                ),
            ))
    }
}
