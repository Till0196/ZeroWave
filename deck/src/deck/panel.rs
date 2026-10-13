// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 下のパネル：表示・折りたたみ・高さ・切り離し。状態は `state.toml` の `panel` に保存する。
//! 切り離している間は別ウィンドウのスレッドがレンダリングし、押されたことは [`Deck::panel_said`] へ返る。

use super::*;

use ui::guide::panel::{self, Feed, Pressed, Said, Tab};

/// 操作が止まってから少し待って保存する（ドラッグしている間に書き続けない）。
const SAVE_AFTER: Duration = Duration::from_millis(600);

const KEEP_VIDEO: f32 = 120.0;

fn rows_of(panel: &crate::config::state::Panel) -> u32 {
    panel.rows.unwrap_or(2).clamp(1, ui::theme::mini::MOST_ROWS)
}

impl Deck {
    pub(super) fn panel_height(&self) -> f32 {
        if self.scene.strip.is_none() || self.panel.detached {
            return 0.0;
        }
        panel::height(rows_of(&self.panel), self.panel.folded)
    }

    fn most_rows(&self, whole: egui::Rect) -> u32 {
        let others = self.chrome() - self.panel_height();
        panel::rows_docked(whole.height() - others - KEEP_VIDEO)
    }

    fn panel_tabs(&mut self) -> Vec<Tab> {
        let loaded = Arc::as_ptr(&crate::plugins::loaded()) as usize;
        match &self.panel_tabs {
            Some((made, tabs)) if *made == loaded => tabs.clone(),
            _ => {
                let tabs = panel::tabs_of(&crate::plugin_ui::panels());
                self.panel_tabs = Some((loaded, tabs.clone()));
                tabs
            }
        }
    }

    pub(super) fn show_panel(&mut self, ui: &mut egui::Ui, whole: egui::Rect) -> Option<Pressed> {
        let most_rows = self.most_rows(whole);
        let tabs = self.panel_tabs();
        let Self {
            scene,
            plugin_ui,
            plugin_scrolls,
            plugin_told,
            theme,
            panel: kept,
            ..
        } = self;
        let banded = scene.strip.as_ref()?;
        let told = Arc::as_ptr(&banded.plugins) as usize;
        if *plugin_told != told {
            *plugin_told = told;
            plugin_ui.tell((*banded.plugins).clone());
        }
        let shown = panel::Panel {
            strip: super::size::strip_of(banded, theme, rows_of(kept)),
            tab: panel::chosen(&tabs, &kept.tab),
            tabs: &tabs,
            folded: kept.folded,
            detached: false,
            detail: banded.detail.as_deref(),
            most_rows,
        };
        let mut plugins = panel::Plugins {
            host: plugin_ui,
            scrolls: plugin_scrolls,
            guide: &banded.guide,
            look: &theme.settings,
        };
        panel::show(ui, &shown, &mut plugins)
    }

    pub(super) fn set_mini(&mut self, on: bool) {
        self.mini = on;
        if on {
            self.show_bottom();
        }
        self.say(ToStage::Wants {
            guide: false,
            info: false,
            mini: on,
        });
        if self.panel.detached {
            if on {
                self.open_panel_window();
            } else {
                let _ = self.to_panes.send(ToPanes::ShutPanel);
            }
        }
    }

    fn open_panel_window(&self) {
        let size = self.panel.size.map_or_else(
            || {
                (
                    panel::WINDOW_WIDTH,
                    panel::window_height(rows_of(&self.panel)),
                )
            },
            |(w, h)| (w as f32, h as f32),
        );
        let _ = self.to_panes.send(ToPanes::Open(Which::Panel(size)));
        self.forward_panel();
    }

    pub(super) fn forward_panel(&self) {
        if !self.mini || !self.panel.detached {
            return;
        }
        let Some(banded) = &self.scene.strip else {
            return;
        };
        let _ = self.to_panes.send(ToPanes::Panel(Box::new(Feed {
            banded: banded.clone(),
            tab: self.panel.tab.clone(),
        })));
    }

    pub(super) fn open_plugin_panel(&mut self, stem: &str, id: &str) {
        self.panel.tab = Tab::Plugin {
            stem: stem.to_owned(),
            id: id.to_owned(),
            title: String::new(),
        }
        .key();
        self.panel.folded = false;
        self.remember_panel();
        if self.mini {
            self.forward_panel();
        } else {
            self.set_mini(true);
        }
    }

    pub(super) fn panel_pressed(&mut self, pressed: Pressed) {
        match pressed {
            Pressed::Strip(ui::guide::mini::Pressed::Channel(key)) => {
                self.say(ToStage::Act(Act::Tune(key)));
            }
            Pressed::Strip(ui::guide::mini::Pressed::Detail(key, programme)) => {
                let _ = self.to_panes.send(ToPanes::Detail(key, programme));
            }
            Pressed::Strip(ui::guide::mini::Pressed::Band(band)) => {
                self.say(ToStage::Act(Act::Band(band)));
            }
            Pressed::Strip(ui::guide::mini::Pressed::Shift(hours)) => {
                self.say(ToStage::Act(Act::StripShift(hours)));
            }
            Pressed::Tab(key) => {
                self.panel.tab = if key == Tab::Guide.key() {
                    String::new()
                } else {
                    key
                };
                self.panel.folded = false;
                self.remember_panel();
                self.forward_panel();
            }
            Pressed::Detach => {
                self.panel.detached = true;
                self.remember_panel();
                self.open_panel_window();
            }
            Pressed::Dock => {
                self.panel.detached = false;
                self.window_rows = None;
                self.remember_panel();
                let _ = self.to_panes.send(ToPanes::ShutPanel);
            }
            Pressed::Fold => {
                self.panel.folded = !self.panel.folded;
                self.remember_panel();
            }
            Pressed::Close => self.set_mini(false),
            Pressed::Rows(rows) => {
                if self.panel.detached {
                    self.window_rows = Some(rows);
                } else {
                    self.panel.rows = Some(rows);
                    self.remember_panel();
                }
            }
            Pressed::OpenPage(stem, page) => {
                let _ = self
                    .to_panes
                    .send(ToPanes::Open(Which::PluginPage(stem, page)));
            }
        }
        self.tell_rows();
    }

    pub fn panel_said(&mut self, said: Said) {
        match said {
            Said::Pressed(pressed) => self.panel_pressed(pressed),
            Said::Rows(rows) => {
                if self.panel.detached {
                    self.window_rows = Some(rows);
                    self.tell_rows();
                }
            }
            Said::Size(size) => {
                if self.panel.size != Some(size) {
                    self.panel.size = Some(size);
                    self.remember_panel();
                }
            }
            Said::Closed { size, by_hand } => {
                self.panel.size = Some(size);
                if by_hand && self.panel.detached {
                    self.panel.detached = false;
                    self.window_rows = None;
                    self.tell_rows();
                }
                self.remember_panel();
            }
        }
    }

    pub(super) fn tell_rows(&mut self) {
        let rows = if self.panel.detached {
            self.window_rows.unwrap_or_else(|| rows_of(&self.panel))
        } else {
            rows_of(&self.panel)
        };
        if self.told_rows != Some(rows) {
            self.told_rows = Some(rows);
            self.say(ToStage::Act(Act::StripRows(rows)));
        }
    }

    fn remember_panel(&mut self) {
        self.panel_changed = Some(Instant::now());
    }

    pub(super) fn save_panel(&mut self, ctx: &egui::Context) {
        let Some(at) = self.panel_changed else {
            return;
        };
        if at.elapsed() < SAVE_AFTER {
            ctx.request_repaint_after(SAVE_AFTER);
            return;
        }
        self.panel_changed = None;
        let panel = self.panel.clone();
        crate::config::update_state(move |state| state.panel = panel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_kept_in_range() {
        let mut kept = crate::config::state::Panel::default();
        assert_eq!(rows_of(&kept), 2);
        kept.rows = Some(4);
        assert_eq!(rows_of(&kept), 4);
        kept.rows = Some(0);
        assert_eq!(rows_of(&kept), 1);
        kept.rows = Some(1000);
        assert_eq!(rows_of(&kept), ui::theme::mini::MOST_ROWS);
    }
}
