// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! 別ウィンドウに届いたウィンドウイベントと、設定ウィンドウのボタン、ウィンドウを閉じる、テーマを適用する。

use super::*;

impl Screen {
    pub(super) fn event(&mut self, id: WindowId, event: WindowEvent) {
        let Some(sub) = self.subs.get_mut(&id) else {
            return;
        };
        let window = sub.window.clone();
        sub.face.on_event(&window, &event);
        // 掴んで動かしている間はレンダリングしない。ただし上や左の縁をドラッグして大きさを変えるときも
        // `Moved` が来るので、大きさが変わっている間は動かしているとは見ない。
        if matches!(event, WindowEvent::Moved(_)) {
            let resizing = sub
                .resized_at
                .is_some_and(|at| at.elapsed() < crate::window::sub::MOVING_HOLD);
            if !resizing {
                sub.moving_until = Some(Instant::now() + crate::window::sub::MOVING_HOLD);
            }
            return;
        }
        sub.dirty = true;
        match event {
            WindowEvent::CloseRequested => self.shut(id),
            // 大きさはレンダリングの前にウィンドウと突き合わせる（`Resized` が来ないウィンドウがあるため）。
            WindowEvent::Resized(_) => {
                sub.size = (0, 0);
                sub.resized_at = Some(Instant::now());
                sub.hurry = true;
                sub.moving_until = None;
            }
            WindowEvent::DroppedFile(path) => {
                let at = self.drop_at.remove(&id).flatten();
                if let Which::Plugin(plugin) | Which::PluginPage(plugin, _) = &sub.what {
                    self.settings_page.drops.push(ui::panes::settings::Dropped {
                        plugin: plugin.clone(),
                        path,
                        at,
                    });
                }
            }
            _ => {}
        }
    }

    /// 設定ウィンドウのボタン。適用と OK は変わったキーだけを `zerowave.toml` へ書き、メインスレッドへ渡す。
    pub(super) fn settings_pressed(&mut self, pressed: ui::panes::settings::Pressed) {
        use ui::panes::settings::Pressed;
        match pressed {
            Pressed::Cancel => {
                self.edited = self.loaded.clone();
                let active = crate::config::load_state().profile;
                self.settings_page.profile = active.clone();
                self.settings_page.active = active;
            }
            // 編集するプロファイルを選んだ。使う物は替えない。
            Pressed::Profile(name) => {
                self.settings_page.profile = name;
            }
            Pressed::Scan => {
                // 直したばかりの設定は先に書く（でないと `channels.toml` と次の起動で対応しない）。
                let unsaved = self.edited.lines != self.loaded.lines
                    || self
                        .edited
                        .sources
                        .iter()
                        .zip(self.loaded.sources.iter())
                        .any(|(now, was)| now.tuner_settings != was.tuner_settings)
                    || self.edited.sources.len() != self.loaded.sources.len();
                if unsaved {
                    self.settings_pressed(Pressed::Apply);
                }
                // チューナーの受信方式は、適用が再生スレッドに届くのを待たずにここで設定する（チャンネルスキャンが使う）。
                zwcore::input::source::coaxd::set_tuner_settings(
                    self.edited
                        .sources
                        .iter()
                        .filter(|source| source.kind == "coaxd")
                        .flat_map(|source| &source.tuner_settings)
                        .filter(|(_, setting)| !setting.is_empty())
                        .map(|(id, setting)| (id.clone(), setting.to_core()))
                        .collect(),
                );
                self.scan_stop
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                self.scan_keep
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let legs = self.scan_legs();
                if legs.is_empty() {
                    if let Ok(mut scanning) = self.scanning.lock() {
                        scanning.done_with = Some(Err(String::from(
                            "スキャンできる受信方式がありません。受信方式の選択と、coaxd のチューナーを確認してください。",
                        )));
                    }
                } else {
                    if let Ok(mut scanning) = self.scanning.lock() {
                        *scanning = crate::scan::Scanning::default();
                    }
                    let job = crate::scan::Job {
                        legs,
                        profile: crate::config::channels_profile(&self.settings_page.profile)
                            .to_owned(),
                        options: zwcore::scan::Options::default(),
                        stop: self.scan_stop.clone(),
                        keep: self.scan_keep.clone(),
                    };
                    let into = self.scanning.clone();
                    let wake = self.to.clone();
                    let requests = self.requests.clone();
                    let proxy = self.proxy.clone();
                    let spawned = std::thread::Builder::new()
                        .name(String::from("scan"))
                        .spawn(move || {
                            let ask = |request: Request| {
                                let _ = requests.send(request);
                                let _ = proxy.send_event(Wake);
                            };
                            crate::scan::run(
                                job,
                                &into,
                                &crate::scan::Hooks {
                                    wake: &|| {
                                        let _ = wake.send(ToPanes::Repaint);
                                    },
                                    yield_tuner: &|yielding| {
                                        ask(Request::YieldTuner(yielding.then_some(
                                            "チャンネルスキャンのため、視聴を停止しました。",
                                        )))
                                    },
                                    finished: &|services| ask(Request::Scanned(services)),
                                },
                            );
                        });
                    if let Err(error) = spawned {
                        tracing::warn!("スキャンのスレッドを起動できない: {error}");
                    }
                }
                self.open(Which::Scan);
            }
            Pressed::AskDeleteScan(plan) => {
                use ui::panes::settings::scan_results;
                let profile =
                    crate::config::channels_profile(&self.settings_page.profile).to_owned();
                let Some(gone) = scan_results::results_of(&profile).get(&plan).cloned() else {
                    return;
                };
                let after = (profile != ui::panes::settings::ALL).then(|| {
                    scan_results::results_of("").get(&plan).map_or_else(
                        || String::from("なし"),
                        |had| format!("all の結果（{had}）"),
                    )
                });
                self.settings_page.scan_delete = Some(scan_results::Deleting {
                    profile,
                    plan,
                    gone,
                    after,
                });
                self.open(Which::ScanDelete);
            }
            Pressed::DeleteScan(go) => {
                let deleting = self.settings_page.scan_delete.take();
                self.shut_which(|what| matches!(what, Which::ScanDelete));
                if go && let Some(deleting) = deleting {
                    crate::config::delete_channels_line(&deleting.profile, &deleting.plan);
                    self.scan_results_changed();
                }
            }
            Pressed::OpenScanCopy => {
                use ui::panes::settings::scan_results;
                let to = crate::config::channels_profile(&self.settings_page.profile).to_owned();
                let sources: Vec<String> = std::iter::once(ui::panes::settings::ALL.to_owned())
                    .chain(
                        self.edited
                            .profiles
                            .iter()
                            .map(|profile| profile.name.clone()),
                    )
                    .filter(|name| *name != to)
                    .collect();
                let results: std::collections::BTreeMap<String, _> = sources
                    .iter()
                    .chain(std::iter::once(&to))
                    .map(|name| (name.clone(), scan_results::results_of(name)))
                    .collect();
                let from = sources
                    .iter()
                    .find(|name| results.get(*name).is_some_and(|had| !had.is_empty()))
                    .or_else(|| sources.first())
                    .cloned()
                    .unwrap_or_default();
                self.settings_page.scan_copy = Some(scan_results::Copying {
                    to,
                    sources,
                    from,
                    chosen: Vec::new(),
                    results,
                });
                self.open(Which::ScanCopy);
            }
            Pressed::CopyScan(go) => {
                let copying = self.settings_page.scan_copy.take();
                self.shut_which(|what| matches!(what, Which::ScanCopy));
                if go && let Some(copying) = copying {
                    let offered = copying.offered();
                    let plans: Vec<String> = copying
                        .chosen
                        .iter()
                        .filter(|plan| offered.contains(&plan.as_str()))
                        .cloned()
                        .collect();
                    crate::config::copy_channels_lines(&copying.from, &copying.to, &plans);
                    self.scan_results_changed();
                }
            }
            // チューナーの一覧は応答まで返らないので別のスレッドで。
            Pressed::AddPlugin => {
                let told = self.settings_page.plugin_told.clone();
                let wake = self.to.clone();
                let requests = self.requests.clone();
                let proxy = self.proxy.clone();
                let spawned = std::thread::Builder::new()
                    .name(String::from("plugin"))
                    .spawn(move || {
                        let Some(path) = crate::window::opening::pick_plugin() else {
                            return;
                        };
                        let result = crate::plugins::add(&path);
                        if let Ok((_, streams)) = &result {
                            let _ = requests.send(Request::Replugged(*streams));
                            let _ = proxy.send_event(Wake);
                        }
                        if let Ok(mut told) = told.lock() {
                            *told = Some(result.map(|(name, _)| name));
                        }
                        let _ = wake.send(ToPanes::Repaint);
                    });
                if let Err(error) = spawned {
                    tracing::warn!("プラグイン選択のスレッドを起動できない: {error}");
                }
            }
            // 保存先で動いている coaxd を替えるときだけ、先に確認する（「coaxd の更新」のウィンドウ）。
            Pressed::PutCoaxd => {
                let coaxd = self.settings_page.coaxd.clone();
                match coaxd.lock() {
                    Ok(mut slot) if !slot.busy => {
                        slot.busy = true;
                        slot.told = None;
                    }
                    _ => return,
                }
                let endpoint = ui::panes::settings::coaxd::endpoint_of(&self.edited);
                let wake = self.to.clone();
                let spawned = std::thread::Builder::new()
                    .name(String::from("coaxd put"))
                    .spawn(move || {
                        let picked = crate::window::opening::pick_program("coaxd を選ぶ");
                        let open = picked.is_some_and(|path| put_coaxd(&path, &endpoint, &coaxd));
                        if open {
                            let _ = wake.send(ToPanes::Open(Which::CoaxdUpdate));
                        } else if let Ok(mut slot) = coaxd.lock() {
                            slot.busy = false;
                            slot.looked = None;
                        }
                        let _ = wake.send(ToPanes::Repaint);
                    });
                if let Err(error) = spawned {
                    tracing::warn!("coaxd 選択のスレッドを起動できない: {error}");
                    if let Ok(mut slot) = self.settings_page.coaxd.lock() {
                        slot.busy = false;
                    }
                }
            }
            // 確認を先に取ってから閉じる（閉じるのはキャンセルと同じ処理）。
            Pressed::UpdateCoaxd(go) => {
                let pending = if go {
                    self.settings_page
                        .coaxd
                        .lock()
                        .ok()
                        .and_then(|mut slot| slot.pending.take())
                } else {
                    None
                };
                if let Some(id) = self
                    .subs
                    .iter()
                    .find(|(_, sub)| matches!(sub.what, Which::CoaxdUpdate))
                    .map(|(id, _)| *id)
                {
                    self.shut(id);
                } else {
                    self.drop_coaxd_update();
                }
                let Some(pending) = pending else {
                    return;
                };
                let coaxd = self.settings_page.coaxd.clone();
                let wake = self.to.clone();
                let requests = self.requests.clone();
                let proxy = self.proxy.clone();
                let spawned = std::thread::Builder::new()
                    .name(String::from("coaxd update"))
                    .spawn(move || {
                        let ask = |request: Request| {
                            let _ = requests.send(request);
                            let _ = proxy.send_event(Wake);
                        };
                        let told = update_coaxd(&pending, &ask);
                        if let Ok(mut slot) = coaxd.lock() {
                            slot.told = Some(told);
                            slot.busy = false;
                            slot.looked = None;
                        }
                        let _ = wake.send(ToPanes::Repaint);
                    });
                if let Err(error) = spawned {
                    tracing::warn!("coaxd を更新するスレッドを起動できない: {error}");
                }
            }
            Pressed::CaptionShown(shown) => {
                self.loaded.caption.shown = shown.clone();
                let _ = self.requests.send(Request::Captions(shown));
                let _ = self.proxy.send_event(Wake);
            }
            Pressed::PickFont {
                target,
                place,
                current,
            } => {
                self.settings_page.font_pick = Some(ui::panes::settings::font_pick::Pick::new(
                    target, place, &current,
                ));
                for sub in self.subs.values_mut() {
                    if matches!(sub.what, Which::FontPick) {
                        sub.dirty = true;
                    }
                }
                self.open(Which::FontPick);
            }
            // ウィンドウは書いてから閉じる（先に閉じると ✕ と同じくキャンセルになる）。
            Pressed::Font(done) => {
                let picked = self.settings_page.font_pick.take();
                if let Some(id) = self
                    .subs
                    .iter()
                    .find(|(_, sub)| matches!(sub.what, Which::FontPick))
                    .map(|(id, _)| *id)
                {
                    self.shut(id);
                }
                let Some(pick) = picked else {
                    return;
                };
                if let Some(write) =
                    ui::panes::settings::font_pick::settle(&pick.target, done, &mut self.edited)
                {
                    crate::config::plugins::put(&write.plugin, &write.key, write.value.as_deref());
                    let streams = crate::plugins::named(&write.plugin)
                        .is_some_and(|plugin| plugin.has("stream"));
                    let _ = self.requests.send(Request::Replugged(streams));
                    let _ = self.proxy.send_event(Wake);
                }
                for sub in self.subs.values_mut() {
                    if matches!(sub.what, Which::Settings | Which::Plugin(_)) {
                        sub.dirty = true;
                    }
                }
            }
            Pressed::BrowseFolder(key, current) => {
                let browsed = self.settings_page.browsed.clone();
                let wake = self.to.clone();
                let start = if current.trim().is_empty() {
                    Some(crate::capture::folder_of(
                        "",
                        if key == "record.folder" {
                            crate::capture::Folder::Videos
                        } else {
                            crate::capture::Folder::Pictures
                        },
                    ))
                } else {
                    Some(std::path::PathBuf::from(current.trim()))
                };
                let spawned = std::thread::Builder::new()
                    .name(String::from("folder"))
                    .spawn(move || {
                        let Some(path) =
                            crate::window::opening::pick_folder("保存先フォルダ", start)
                        else {
                            return;
                        };
                        if let Ok(mut browsed) = browsed.lock() {
                            browsed.push((String::new(), key.to_owned(), path));
                        }
                        let _ = wake.send(ToPanes::Repaint);
                    });
                if let Err(error) = spawned {
                    tracing::warn!("フォルダー選択のスレッドを起動できない: {error}");
                }
            }
            Pressed::Browse {
                plugin,
                key,
                label,
                folder,
            } => {
                let browsed = self.settings_page.browsed.clone();
                let wake = self.to.clone();
                let spawned = std::thread::Builder::new()
                    .name(String::from("program"))
                    .spawn(move || {
                        let picked = if folder {
                            crate::window::opening::pick_folder(&label, None)
                        } else {
                            crate::window::opening::pick_program(&label)
                        };
                        let Some(path) = picked else {
                            return;
                        };
                        if let Ok(mut browsed) = browsed.lock() {
                            browsed.push((plugin, key, path));
                        }
                        let _ = wake.send(ToPanes::Repaint);
                    });
                if let Err(error) = spawned {
                    tracing::warn!("プログラム選択のスレッドを起動できない: {error}");
                }
            }
            Pressed::Reveal(path) => crate::window::opening::reveal(&path),
            Pressed::Replugged(streams) | Pressed::Settled(streams) => {
                let _ = self.requests.send(Request::Replugged(streams));
                let _ = self.proxy.send_event(Wake);
            }
            // チューナーの状態は開くたびに問い合わせ直す（coaxd を起動して待つことがある）。
            Pressed::OpenTuners => {
                ui::panes::settings::ask_boards(&mut self.settings_page, &self.edited);
                self.open(Which::Tuners);
            }
            Pressed::OpenPlugin(name) => match crate::plugin_ui::settings_page_of(&name) {
                Some(page) => self.open(Which::PluginPage(name, page)),
                None => self.open(Which::Plugin(name)),
            },
            Pressed::OpenPluginPage(stem, page) => self.open(Which::PluginPage(stem, page)),
            Pressed::OpenFiveOne => {
                self.settings_page.five_one = None;
                self.open(Which::FiveOne);
            }
            Pressed::OpenTwentyTwo => {
                self.settings_page.twenty_two = None;
                self.open(Which::TwentyTwo);
            }
            Pressed::Boards => {
                ui::panes::settings::ask_boards(&mut self.settings_page, &self.edited);
            }
            Pressed::Close => {}
            // ウィンドウごとにクリップボードの扱いが違うので設定ウィンドウから。
            Pressed::CopyKey(written) => {
                if let Some(sub) = self
                    .subs
                    .values()
                    .find(|sub| matches!(sub.what, Which::Settings))
                {
                    sub.face.copy_text(written);
                }
            }
            Pressed::Check => {
                let checks = self.settings_page.checks.clone();
                let sources = self.edited.sources.clone();
                if let Ok(mut checks) = checks.lock() {
                    checks.clear();
                }
                let wake = self.to.clone();
                std::thread::Builder::new()
                    .name(String::from("check"))
                    .spawn(move || {
                        for source in sources {
                            let result = check_source(&source);
                            if let Ok(mut checks) = checks.lock() {
                                checks.insert(source.label.clone(), result);
                            }
                            let _ = wake.send(ToPanes::Repaint);
                        }
                    })
                    .ok();
            }
            Pressed::Apply | Pressed::Ok => {
                let changes = crate::config::changes(&self.loaded, &self.edited);
                if changes.is_empty() {
                    return;
                }
                // 使っているプロファイルは替えない。名前を変えたなら追い、消したなら all に戻す。
                let active = crate::config::load_state().profile;
                let follow = follow_active(&active, &self.loaded.profiles, &self.edited.profiles);
                let renamed = renamed_profiles(&self.loaded.profiles, &self.edited.profiles);
                if !changes.is_empty() {
                    // プラグインの構成が替わるなら、本体が読み直し終えるまで「読み込んでいます…」。
                    if self.loaded.data.plugins != self.edited.data.plugins
                        || self.loaded.data.disabled != self.edited.data.disabled
                    {
                        crate::plugins::expect_reload();
                    }
                    match crate::config::save_changes(&changes) {
                        Ok(()) => {
                            self.loaded = self.edited.clone();
                            // 自分で書いた分は「外で直された」ではない。
                            self.file_seen = crate::config::modified();
                        }
                        Err(why) => {
                            tracing::warn!("設定を保存できない: {why}");
                            return;
                        }
                    }
                }
                for (from, to) in &renamed {
                    crate::config::rename_channels_profile(from, to);
                }
                if !renamed.is_empty() {
                    self.settings_page.reach = None;
                    self.settings_page.results = None;
                    self.settings_page.channels = None;
                }
                if let Some(name) = follow {
                    self.settings_page.active = name.clone();
                    crate::config::update_state(move |state| state.profile = name);
                }
                let _ = self
                    .requests
                    .send(Request::Settings(Box::new(self.loaded.clone())));
                let _ = self.proxy.send_event(Wake);
            }
        }
    }

    pub(super) fn shut(&mut self, id: WindowId) {
        // イベントループが持つウィンドウの参照を手放してもらう。こちらが捨てるだけではウィンドウが残る。
        let _ = self.requests.send(Request::Closed(id));
        let _ = self.proxy.send_event(Wake);
        let closed_size = self.subs.get(&id).map_or((0, 0), |sub| {
            let size = sub
                .window
                .inner_size()
                .to_logical::<f64>(sub.window.scale_factor());
            (size.width.round() as u32, size.height.round() as u32)
        });
        self.drop_at.remove(&id);
        match self.subs.remove(&id).map(|sub| sub.release().what) {
            Some(Which::Stream) => {
                self.stream.open = false;
                self.info = None;
            }
            Some(Which::Decoder) => self.decoding = None,
            Some(Which::Search) => {
                let _ = self.guide_to.send(ToGuide::Search(None));
            }
            Some(Which::Favourites) => self.settings_page.favourites = None,
            // ✕ で閉じたのはキャンセル（決めたときは状態を先に取ってある）。
            Some(Which::FontPick) => self.settings_page.font_pick = None,
            Some(Which::CoaxdUpdate) => self.drop_coaxd_update(),
            Some(Which::ScanDelete) => self.settings_page.scan_delete = None,
            Some(Which::ScanCopy) => self.settings_page.scan_copy = None,
            Some(Which::Settings) => self.shut_font_pick(|target| {
                matches!(target, ui::panes::settings::font_pick::Target::Setting(_))
            }),
            Some(Which::PluginPage(stem, _)) => {
                self.settings_page.plugin_drafts.remove(&stem);
            }
            Some(Which::Plugin(name)) => self.shut_font_pick(|target| {
                matches!(target, ui::panes::settings::font_pick::Target::Plugin { plugin, .. } if *plugin == name)
            }),
            Some(Which::Panel(_)) => {
                self.panel_rows = None;
                self.panel_size = None;
                let _ = self
                    .requests
                    .send(Request::Panel(ui::guide::panel::Said::Closed {
                        size: closed_size,
                        by_hand: !self.panel_quiet,
                    }));
                let _ = self.proxy.send_event(Wake);
            }
            Some(Which::OneSeg) => {
                self.popup_area = None;
                let _ = self.to_broadcast.try_send(ToBroadcast::Popup(None));
            }
            _ => {}
        }
        self.tell_wants();
    }

    /// その種類の別ウィンドウが開いていれば閉じる。
    fn shut_which(&mut self, which: impl Fn(&Which) -> bool) {
        if let Some(id) = self
            .subs
            .iter()
            .find(|(_, sub)| which(&sub.what))
            .map(|(id, _)| *id)
        {
            self.shut(id);
        }
    }

    fn scan_results_changed(&mut self) {
        self.settings_page.reach = None;
        self.settings_page.channels = None;
        self.settings_page.results = None;
        if let Ok(mut listed) = self.settings_page.listed.lock() {
            listed.profile = None;
        }
        let _ = self.requests.send(Request::Scanned(Vec::new()));
        let _ = self.proxy.send_event(Wake);
    }

    fn drop_coaxd_update(&mut self) {
        if let Ok(mut slot) = self.settings_page.coaxd.lock()
            && let Some(pending) = slot.pending.take()
        {
            crate::coaxd::discard(&pending.staged);
            slot.busy = false;
        }
    }

    fn shut_font_pick(&mut self, whose: impl Fn(&ui::panes::settings::font_pick::Target) -> bool) {
        if !self
            .settings_page
            .font_pick
            .as_ref()
            .is_some_and(|pick| whose(&pick.target))
        {
            return;
        }
        self.settings_page.font_pick = None;
        if let Some(id) = self
            .subs
            .iter()
            .find(|(_, sub)| matches!(sub.what, Which::FontPick))
            .map(|(id, _)| *id)
        {
            self.shut(id);
        }
    }

    pub(super) fn wear(&mut self, mode: ui::theme::Mode) {
        if self.theme.mode == mode {
            return;
        }
        self.theme = ui::theme::Theme::of(mode);
        for sub in self.subs.values_mut() {
            if sub.what.paper() {
                sub.face.repaper(mode);
            }
            sub.dirty = true;
        }
    }

    pub(super) fn tell_wants(&mut self) {
        let guide = self
            .subs
            .values()
            .any(|sub| matches!(sub.what, Which::Guide));
        let info = self
            .subs
            .values()
            .any(|sub| matches!(sub.what, Which::Stream));
        let decoder = self
            .subs
            .values()
            .any(|sub| matches!(sub.what, Which::Decoder));
        if self.wanted == (guide, info, decoder) {
            return;
        }
        self.wanted = (guide, info, decoder);
        let _ = self.say.send(Chose::Wants {
            guide,
            info,
            decoder,
        });
    }
}

/// 選んだ coaxd を確かめて保存先に入れる。保存先で動いている coaxd を替えるなら入れずに
/// 確認を待つ状態にして真を返す。
fn put_coaxd(
    path: &std::path::Path,
    endpoint: &str,
    coaxd: &ui::panes::settings::coaxd::Coaxd,
) -> bool {
    use ui::panes::settings::coaxd::{Pending, Told};
    let tell = |told: Told| {
        if let Ok(mut slot) = coaxd.lock() {
            slot.told = Some(told);
        }
    };
    let staged = match crate::coaxd::stage(path) {
        Ok(staged) => staged,
        Err(why) => {
            tell(Told::Failed(why));
            return false;
        }
    };
    let seen = crate::coaxd::look(endpoint);
    if let (Some(running), Some(true)) = (&seen.running, seen.ours) {
        let now = [
            running.version.as_str(),
            crate::coaxd::shown_arch(&running.arch),
        ]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("・");
        if let Ok(mut slot) = coaxd.lock() {
            slot.pending = Some(Pending {
                staged,
                now,
                pid: running.pid,
            });
        }
        return true;
    }
    match crate::coaxd::swap(&staged) {
        Ok(_) if seen.present => tell(Told::Done(String::from("coaxd を更新しました"))),
        Ok(_) if staged.picked != crate::coaxd::file_name() && !staged.picked.is_empty() => {
            tell(Told::Done(format!(
                "{} を {} として追加しました",
                staged.picked,
                crate::coaxd::file_name()
            )));
        }
        Ok(_) => tell(Told::Done(String::from("coaxd を追加しました"))),
        Err(why) => {
            crate::coaxd::discard(&staged);
            tell(Told::Failed(why));
        }
    }
    false
}

/// 保存先で動いている coaxd を替える。先にファイルを入れ替え、coaxd が使われなくなって
/// 終わるのを待ち、見ていた局へ戻す。終わらなければファイルを前に戻す。
fn update_coaxd(
    pending: &ui::panes::settings::coaxd::Pending,
    ask: &dyn Fn(Request),
) -> ui::panes::settings::coaxd::Told {
    use ui::panes::settings::coaxd::Told;
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(40);
    let old = match crate::coaxd::swap(&pending.staged) {
        Ok(old) => old,
        Err(why) => {
            crate::coaxd::discard(&pending.staged);
            return Told::Failed(why);
        }
    };
    ask(Request::YieldTuner(Some(
        "coaxd を更新するため、視聴を停止しました。",
    )));
    let began = std::time::Instant::now();
    let gone = loop {
        let running = pending
            .pid
            .is_some_and(zwcore::input::source::coaxd::still_running);
        if !running {
            break true;
        }
        if began.elapsed() >= PATIENCE {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    let told = if gone {
        Told::Done(String::from("coaxd を更新しました"))
    } else {
        crate::coaxd::restore(&pending.staged, old.as_deref());
        crate::coaxd::discard(&pending.staged);
        Told::Kept(String::from(
            "更新失敗（動作中のcoaxd を終了できませんでした）",
        ))
    };
    ask(Request::YieldTuner(None));
    told
}
