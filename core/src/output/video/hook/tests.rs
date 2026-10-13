// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

use super::run::{Verdict, judge_chain};
use super::*;

fn key() -> Key {
    Key {
        width: 1440,
        height: 1080,
        depth: 8,
        shape: Shape::Planar,
        interlaced: true,
        aspect: [16, 9],
        rate: [30000, 1001],
        transfer: "sdr",
        target: [1920, 1080],
        display: 60_000,
    }
}

fn paused(ms: f64, budget: f64) -> Paused {
    Paused {
        name: String::from("p"),
        deinterlace: false,
        ms,
        budget,
        pass_times: Vec::new(),
    }
}

#[test]
fn a_longer_interval_retries_what_was_paused() {
    let mut hooks = Hooks::new();
    hooks.key = Some(key());
    hooks.set_interval(1000.0 / 59.94);
    hooks.paused.push(paused(20.0, 8.34));
    hooks.paused_interval = hooks.interval_ms;
    hooks.set_interval(17.0);
    assert!(hooks.key.is_some());
    // 24p に戻った：ビルドし直して測り直す。
    hooks.set_interval(1000.0 / 23.976);
    assert!(hooks.key.is_none());
    assert!((hooks.budget() - 1000.0 / 23.976 * AFFORDABLE).abs() < 1e-9);
}

#[test]
fn only_the_heaviest_stage_is_dropped() {
    assert_eq!(
        judge_chain(&[(3.0, false), (4.0, false)], 8.34),
        Verdict::Fits
    );
    assert_eq!(
        judge_chain(&[(3.0, false), (9.0, false), (2.0, false)], 8.34),
        Verdict::Drop(1)
    );
    // 一つずつは収まっても、合計が越えれば重い方を外す。
    assert_eq!(
        judge_chain(&[(5.0, false), (6.0, false)], 8.34),
        Verdict::Drop(1)
    );
}

#[test]
fn kept_stages_are_never_dropped() {
    assert_eq!(
        judge_chain(&[(3.0, false), (9.0, true)], 8.34),
        Verdict::Drop(0)
    );
    assert_eq!(judge_chain(&[(9.0, true)], 8.34), Verdict::KeepOver);
}

#[test]
fn nothing_paused_means_nothing_to_retry() {
    let mut hooks = Hooks::new();
    hooks.key = Some(key());
    hooks.set_interval(1000.0 / 23.976);
    assert!(hooks.key.is_some());
}

fn validates(label: &str, text: &str) {
    let module = naga::front::wgsl::parse_str(text)
        .unwrap_or_else(|error| panic!("{label}: {}", error.emit_to_string(text)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{label}: {}", error.emit_to_string(text)));
}

#[test]
fn the_vertex_and_the_copy_validate() {
    validates("copy", &format!("{VERTEX}{COPY}"));
}

#[test]
fn a_pass_written_to_the_contract_validates() {
    let fragment = format!(
        "{}
@group(0) @binding(0) var<uniform> zw: Zw;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var picture_smp: sampler;
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {{
    return textureSample(picture, picture_smp, uv) * zw.phase;
}}
",
        zv::ZW_WGSL
    );
    validates("fragment", &format!("{VERTEX}{fragment}"));
    let compute = format!(
        "{}
@group(0) @binding(0) var<uniform> zw: Zw;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var picture_smp: sampler;
@group(0) @binding(3) var out: texture_storage_2d<rgba16float, write>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let uv = (vec2<f32>(id.xy) + 0.5) / zw.output_size;
    textureStore(out, vec2<i32>(id.xy), textureSampleLevel(picture, picture_smp, uv, 0.0));
}}
",
        zv::ZW_WGSL
    );
    validates("compute", &compute);
}

#[test]
fn every_named_format_maps_to_a_texture_format() {
    for name in [
        "rgba16float",
        "rgba32float",
        "rgba8unorm",
        "r16float",
        "r32float",
        "rg16float",
        "rg32float",
        "r8unorm",
        "rg8unorm",
        "rgba16f",
    ] {
        let format = format_of(name).expect(name);
        assert_eq!(
            format.block_copy_size(None).map(|b| b as usize),
            zv::texel_bytes(name),
            "{name}"
        );
    }
    assert_eq!(format_of("bgra8"), None);
}
