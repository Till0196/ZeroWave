// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! プラグインのメニューとページ（`ui` hook）。インスタンスはスレッドごとに作る。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zwcore::output::video::hook::Report;
use zwsdk::ui::{
    Channel, Ctx, Event, MenuItem, Page, PausedStatus, Reply, StageStatus, VideoStatus,
};
use zwsdk::video::{At, Format, Program};

const PAGE_EVERY: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shared {
    pub program: Option<Program>,
    pub format: Option<Format>,
    pub report: Option<Report>,
    /// 選局からの表示したフレーム数と、間に合わず出さなかったフレーム数と、直近の間に合わなかったフレーム数。
    pub drawn: u64,
    pub late: u64,
    pub late_recent: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiPlugin {
    pub stem: String,
    pub name: String,
    pub title: String,
    pub settings: Option<String>,
    pub panels: Vec<(String, String)>,
}

pub fn plugins() -> Vec<UiPlugin> {
    crate::plugins::loaded()
        .ui()
        .iter()
        .map(|(plugin, _)| {
            let manifest = plugin.manifest();
            UiPlugin {
                stem: crate::plugins::stem_of(plugin),
                name: manifest.name.clone(),
                title: if manifest.ui.title.is_empty() {
                    manifest.name.clone()
                } else {
                    manifest.ui.title.clone()
                },
                settings: manifest.ui.settings.clone(),
                panels: manifest
                    .ui
                    .panels
                    .iter()
                    .map(|panel| {
                        let title = if panel.title.is_empty() {
                            panel.id.clone()
                        } else {
                            panel.title.clone()
                        };
                        (panel.id.clone(), title)
                    })
                    .collect(),
            }
        })
        .collect()
}

pub fn panels() -> Vec<(String, String, String)> {
    plugins()
        .into_iter()
        .flat_map(|plugin| {
            plugin
                .panels
                .into_iter()
                .map(move |(id, title)| (plugin.stem.clone(), id, title))
        })
        .collect()
}

pub fn settings_page_of(stem: &str) -> Option<String> {
    plugins()
        .into_iter()
        .find(|plugin| plugin.stem == stem)
        .and_then(|plugin| plugin.settings)
}

pub fn program_of(
    guide: &zwcore::guide::Guide,
    programme: &zwcore::guide::Programme,
    recording: bool,
) -> Program {
    let key = programme.key();
    Program {
        genres: programme
            .genres
            .iter()
            .map(|genre| (genre.large, genre.middle))
            .collect(),
        network: key.network,
        service: key.service,
        band: band_name(key.network),
        channel: guide
            .service(key)
            .map(|service| service.name.clone())
            .unwrap_or_default(),
        title: programme.title.clone(),
        recording,
        structure: zwsdk::video::Structure::Unknown,
        minute: guide.now_or_machine().div_euclid(60),
    }
}

pub fn band_name(network: u16) -> String {
    let band = zwcore::guide::Band::of(network);
    if band == zwcore::guide::Band::OTHER {
        String::new()
    } else {
        band.name()
    }
}

pub fn status_for(report: &Report, name: &str) -> VideoStatus {
    let at = |deinterlace: bool| {
        if deinterlace {
            At::Deinterlace
        } else {
            At::Picture
        }
    };
    VideoStatus {
        building: report.building,
        stages: report
            .stages
            .iter()
            .filter(|stage| stage.name == name)
            .map(|stage| StageStatus {
                at: at(stage.deinterlace),
                input: stage.input,
                output: stage.output,
                ms: stage.ms,
                max: stage.max,
                passes: stage.pass_times.clone(),
                over: stage.over,
            })
            .collect(),
        paused: report
            .paused
            .iter()
            .filter(|paused| paused.name == name)
            .map(|paused| PausedStatus {
                at: at(paused.deinterlace),
                ms: paused.ms,
                budget: paused.budget,
            })
            .collect(),
        failed: report
            .failed
            .iter()
            .filter(|(failed, _)| failed == name)
            .map(|(_, why)| why.clone())
            .collect(),
        budget: report.budget,
        rate: report.rate,
        device: report.device.clone(),
        host: report.host.clone(),
        ..VideoStatus::default()
    }
}

pub fn ctx_of(shared: &Shared, plugin: &UiPlugin, guide: Option<&zwcore::guide::Guide>) -> Ctx {
    let mut ctx = Ctx {
        program: shared.program.clone(),
        format: shared.format.clone(),
        video: shared.report.as_ref().map(|report| VideoStatus {
            drawn: shared.drawn,
            late: shared.late,
            late_recent: shared.late_recent,
            ..status_for(report, &plugin.name)
        }),
        ..Ctx::default()
    };
    if let Some(guide) = guide {
        let mut channels: Vec<&zwcore::guide::Service> = guide.channels().collect();
        channels.sort_by_key(|service| service.order());
        ctx.channels = channels
            .iter()
            .map(|service| Channel {
                network: service.network,
                service: service.service,
                name: service.name.clone(),
            })
            .collect();
        ctx.networks = zwcore::guide::Band::all()
            .into_iter()
            .map(zwcore::guide::Band::name)
            .collect();
    }
    ctx
}

#[derive(Default)]
pub struct Instances {
    made_for: usize,
    made: BTreeMap<String, zwsdk::host::UiInstance>,
    broken: BTreeSet<String>,
}

impl std::fmt::Debug for Instances {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Instances")
            .field("made", &self.made.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Instances {
    fn get(&mut self, stem: &str) -> Option<&mut zwsdk::host::UiInstance> {
        let loaded = crate::plugins::loaded();
        let now = Arc::as_ptr(&loaded) as usize;
        if now != self.made_for {
            self.made_for = now;
            self.made.clear();
            self.broken.clear();
        }
        if !self.made.contains_key(stem) && !self.broken.contains(stem) {
            let found = loaded
                .ui()
                .into_iter()
                .find(|(plugin, _)| crate::plugins::stem_of(plugin) == stem);
            match found.map(|(plugin, io)| {
                plugin.ui(zwcore::plugins::logger(plugin.name().to_owned()), io)
            }) {
                Some(Ok(instance)) => {
                    self.made.insert(stem.to_owned(), instance);
                }
                Some(Err(why)) => {
                    tracing::warn!("プラグイン {why}");
                    self.broken.insert(stem.to_owned());
                }
                None => return None,
            }
        }
        self.made.get_mut(stem)
    }

    fn failed(&mut self, stem: &str, why: &str) {
        tracing::warn!("プラグイン {why}");
        self.made.remove(stem);
        self.broken.insert(stem.to_owned());
    }

    pub fn menus(&mut self, shared: &Shared) -> Vec<(UiPlugin, Vec<MenuItem>)> {
        plugins()
            .into_iter()
            .filter_map(|plugin| {
                let ctx = ctx_of(shared, &plugin, None);
                let instance = self.get(&plugin.stem)?;
                match instance.menu(&ctx) {
                    Ok(items) => Some((plugin, items)),
                    Err(why) => {
                        self.failed(&plugin.stem, &why);
                        None
                    }
                }
            })
            .collect()
    }

    pub fn pick(&mut self, stem: &str, id: &str, shared: &Shared) {
        let Some(plugin) = plugins().into_iter().find(|p| p.stem == stem) else {
            return;
        };
        let ctx = ctx_of(shared, &plugin, None);
        let Some(instance) = self.get(stem) else {
            return;
        };
        if let Err(why) = instance.pick(&ctx, id) {
            self.failed(stem, &why);
        }
    }
}

type Picked = Arc<std::sync::Mutex<Vec<(String, String, String, std::path::PathBuf)>>>;

#[derive(Debug, Default)]
pub struct Host {
    instances: Instances,
    pages: BTreeMap<(String, String), (Instant, Result<Page, String>)>,
    pub shared: Shared,
    picked: Picked,
    /// 利用者がファイル選択で選んだパス。コピーできるのはこれだけ。
    allowed: BTreeSet<std::path::PathBuf>,
}

impl Host {
    pub fn page(
        &mut self,
        stem: &str,
        page: &str,
        guide: &zwcore::guide::Guide,
    ) -> Result<Page, String> {
        self.deliver_picked(stem, page, guide);
        let key = (stem.to_owned(), page.to_owned());
        if let Some((at, page)) = self.pages.get(&key)
            && at.elapsed() < PAGE_EVERY
        {
            return page.clone();
        }
        let answer = self.ask_page(stem, page, guide);
        self.pages.insert(key, (Instant::now(), answer.clone()));
        answer
    }

    fn ask_page(
        &mut self,
        stem: &str,
        page: &str,
        guide: &zwcore::guide::Guide,
    ) -> Result<Page, String> {
        let plugin = plugins()
            .into_iter()
            .find(|p| p.stem == stem)
            .ok_or_else(|| format!("プラグイン {stem} が見つかりません"))?;
        let ctx = ctx_of(&self.shared, &plugin, Some(guide));
        let instance = self
            .instances
            .get(stem)
            .ok_or_else(|| format!("{} のページを開けません", plugin.title))?;
        let answer = instance.page(page, &ctx);
        if let Err(why) = &answer {
            self.instances.failed(stem, why);
        }
        answer
    }

    pub fn event(
        &mut self,
        stem: &str,
        page: &str,
        event: &Event,
        guide: &zwcore::guide::Guide,
    ) -> Reply {
        self.pages.retain(|(owner, _), _| owner != stem);
        let Some(plugin) = plugins().into_iter().find(|p| p.stem == stem) else {
            return Reply::default();
        };
        let ctx = ctx_of(&self.shared, &plugin, Some(guide));
        let Some(instance) = self.instances.get(stem) else {
            return Reply::default();
        };
        let reply = match instance.event(page, &ctx, event) {
            Ok(reply) => reply,
            Err(why) => {
                self.instances.failed(stem, &why);
                return Reply::default();
            }
        };
        if let Some(pick) = &reply.pick_file {
            self.ask_file(&plugin, page, pick);
        }
        if !reply.files.is_empty() {
            let said = self.file_ops(stem, &reply.files);
            let _ = self.event(
                stem,
                page,
                &Event {
                    id: String::from("files"),
                    value: said,
                },
                guide,
            );
        }
        reply
    }

    fn ask_file(&self, plugin: &UiPlugin, page: &str, pick: &zwsdk::ui::PickFile) {
        let into = Arc::clone(&self.picked);
        let (stem, page, id) = (plugin.stem.clone(), page.to_owned(), pick.id.clone());
        let (title, filters) = (plugin.title.clone(), pick.filters.clone());
        let spawned = std::thread::Builder::new()
            .name(String::from("plugin-pick"))
            .spawn(move || {
                if let Some(path) = crate::window::opening::pick_file(&title, &filters) {
                    into.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((stem, page, id, path));
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("ファイル選択ダイアログのスレッドを起動できない: {error}");
        }
    }

    fn deliver_picked(&mut self, stem: &str, page: &str, guide: &zwcore::guide::Guide) {
        let ready: Vec<_> = {
            let mut picked = self.picked.lock().unwrap_or_else(|e| e.into_inner());
            let (ready, rest): (Vec<_>, Vec<_>) = picked
                .drain(..)
                .partition(|(owner, at, _, _)| owner == stem && at == page);
            *picked = rest;
            ready
        };
        for (_, _, id, path) in ready {
            self.allowed.insert(path.clone());
            let _ = self.event(
                stem,
                page,
                &Event {
                    id,
                    value: path.to_string_lossy().into_owned(),
                },
                guide,
            );
        }
    }

    /// ファイルをコピー・削除する。マニフェストで宣言したフォルダーの中だけ、コピーするのは選ばれた物だけ。
    fn file_ops(&self, stem: &str, ops: &[zwsdk::ui::FileOp]) -> String {
        let loaded = crate::plugins::loaded();
        let Some((_, io)) = loaded
            .ui()
            .into_iter()
            .find(|(plugin, _)| crate::plugins::stem_of(plugin) == stem)
        else {
            return String::from("プラグインが見つかりません");
        };
        let mut errors = Vec::new();
        for op in ops {
            let (folder, path) = match op {
                zwsdk::ui::FileOp::Copy { folder, path, .. }
                | zwsdk::ui::FileOp::Remove { folder, path } => (folder, path),
            };
            let Some(root) = io.folder(folder) else {
                errors.push(format!("フォルダーが見つかりません：{folder}"));
                continue;
            };
            if !zwsdk::ui::safe_relative(path) {
                errors.push(format!("使えない名前です：{path}"));
                continue;
            }
            let target = root.join(path);
            let done = match op {
                zwsdk::ui::FileOp::Copy { from, .. } => {
                    let from = std::path::PathBuf::from(from);
                    if !self.allowed.contains(&from) {
                        Err(String::from("選ばれていないファイルです"))
                    } else {
                        target
                            .parent()
                            .map_or(Ok(()), std::fs::create_dir_all)
                            .and_then(|()| std::fs::copy(&from, &target).map(|_| ()))
                            .map_err(|error| error.to_string())
                    }
                }
                zwsdk::ui::FileOp::Remove { .. } => {
                    std::fs::remove_file(&target).map_err(|error| error.to_string())
                }
            };
            if let Err(why) = done {
                tracing::warn!(
                    "プラグイン {stem}: {} の操作に失敗: {why}",
                    target.display()
                );
                errors.push(format!("{path}：{why}"));
            }
        }
        errors.join("\n")
    }

    pub fn tell(&mut self, shared: Shared) {
        self.shared = shared;
    }
}

pub fn title_of(stem: &str) -> String {
    plugins()
        .into_iter()
        .find(|p| p.stem == stem)
        .map_or_else(|| crate::plugins::shown_name(stem), |p| p.title)
}
