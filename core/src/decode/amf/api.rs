// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! AMF の C の宣言のうち、使う所だけを Rust に移したもの（`amf/public/include`、MIT）。vtable の関数はインデックスで取り出す。
//! `AMFFactory` は `AMFInterface` を継がない（0 番が `CreateContext`）。他は 0〜2 が
//! `Acquire`・`Release`・`QueryInterface`、3〜12 が `AMFPropertyStorage`。

use std::ffi::c_void;
use std::ptr::NonNull;

use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::core::Interface;

pub type Status = i32;
pub const AMF_OK: Status = 0;
pub const AMF_EOF: Status = 23;
pub const AMF_REPEAT: Status = 24;
pub const AMF_INPUT_FULL: Status = 25;
pub const AMF_RESOLUTION_CHANGED: Status = 26;
pub const AMF_RESOLUTION_UPDATED: Status = 27;
pub const AMF_DECODER_NO_FREE_SURFACES: Status = 35;
pub const AMF_NEED_MORE_INPUT: Status = 44;

pub fn status_name(status: Status) -> &'static str {
    match status {
        0 => "AMF_OK",
        1 => "AMF_FAIL",
        10 => "AMF_NOT_SUPPORTED",
        17 => "AMF_NO_DEVICE",
        23 => "AMF_EOF",
        24 => "AMF_REPEAT",
        25 => "AMF_INPUT_FULL",
        26 => "AMF_RESOLUTION_CHANGED",
        27 => "AMF_RESOLUTION_UPDATED",
        30 => "AMF_CODEC_NOT_SUPPORTED",
        35 => "AMF_DECODER_NO_FREE_SURFACES",
        44 => "AMF_NEED_MORE_INPUT",
        _ => "（名前を定義していない値）",
    }
}

pub const AMF_MEMORY_HOST: i32 = 1;
pub const AMF_MEMORY_DX11: i32 = 3;
pub const AMF_SURFACE_NV12: i32 = 1;
pub const AMF_SURFACE_P010: i32 = 10;
pub const AMF_DX11_0: i32 = 110;
pub const AMF_VARIANT_BOOL: i32 = 1;
pub const AMF_VARIANT_INT64: i32 = 2;
pub const AMF_TS_PRESENTATION: i64 = 0;

pub const fn version(major: u64, minor: u64, release: u64, build: u64) -> u64 {
    (major << 48) | (minor << 32) | (release << 16) | build
}

pub const HEADER_VERSION: u64 = version(1, 5, 2, 0);

/// `AMFVariantStruct`（24 バイト、値で渡す）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub kind: i32,
    pub value: [u64; 2],
}

impl Variant {
    pub fn int(value: i64) -> Self {
        Self {
            kind: AMF_VARIANT_INT64,
            value: [value as u64, 0],
        }
    }

    pub fn boolean(value: bool) -> Self {
        Self {
            kind: AMF_VARIANT_BOOL,
            value: [u64::from(value), 0],
        }
    }
}

pub type Init = unsafe extern "C" fn(version: u64, factory: *mut *mut c_void) -> Status;
pub type QueryVersion = unsafe extern "C" fn(version: *mut u64) -> Status;

pub mod factory {
    pub const CREATE_CONTEXT: usize = 0;
    pub const CREATE_COMPONENT: usize = 1;
}

pub mod interface {
    pub const ACQUIRE: usize = 0;
    pub const RELEASE: usize = 1;
    pub const SET_PROPERTY: usize = 3;
}

pub mod context {
    pub const TERMINATE: usize = 13;
    pub const INIT_DX11: usize = 18;
    pub const ALLOC_BUFFER: usize = 43;
}

pub mod component {
    pub const INIT: usize = 17;
    pub const GET_CAPS: usize = 26;
    pub const TERMINATE: usize = 19;
    pub const DRAIN: usize = 20;
    pub const FLUSH: usize = 21;
    pub const SUBMIT_INPUT: usize = 22;
    pub const QUERY_OUTPUT: usize = 23;
}

pub mod caps {
    pub const GET_ACCELERATION_TYPE: usize = 13;
}

pub const AMF_ACCEL_HARDWARE: i32 = 0;

/// `AMFBuffer`・`AMFSurface` も同じインデックスで持つ。
pub mod data {
    pub const GET_MEMORY_TYPE: usize = 13;
    pub const CONVERT: usize = 15;
    pub const SET_PTS: usize = 19;
    pub const GET_PTS: usize = 20;
}

pub mod buffer {
    pub const GET_NATIVE: usize = 25;
}

pub mod surface {
    pub const GET_FORMAT: usize = 23;
    pub const GET_PLANES_COUNT: usize = 24;
    pub const GET_PLANE_AT: usize = 25;
}

pub mod plane {
    pub const GET_NATIVE: usize = 4;
    pub const GET_H_PITCH: usize = 10;
    pub const GET_V_PITCH: usize = 11;
}

/// # Safety
/// `this` は生きた AMF の物で、`F` はそのインデックスの関数の本当の型。
unsafe fn call<F: Copy>(this: *mut c_void, index: usize) -> F {
    debug_assert_eq!(size_of::<F>(), size_of::<*const c_void>());
    // SAFETY: 呼び出し側が保証するとおり、先頭のポインターが vtable を指す。
    unsafe {
        let table = *this.cast::<*const *const c_void>();
        let slot = *table.add(index);
        std::mem::transmute_copy(&slot)
    }
}

/// vtable のインデックスの関数を、`self.raw()` を先頭に足して呼ぶメソッドにする。引数がポインターの物は
/// `unsafe fn` にする（指す先が有効であることは呼び出し側が保証する）。
macro_rules! slots {
    ($($(#[$meta:meta])* $(@$unsafe:ident)? $vis:vis fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $index:expr;)*) => {$(
        $(#[$meta])*
        $vis $($unsafe)? fn $name(&self $(, $arg: $ty)*) -> $ret {
            // SAFETY: インデックスと型は `amf/public/include` の宣言どおりで、`self` は生きている。
            unsafe {
                let function: unsafe extern "system" fn(*mut c_void $(, $ty)*) -> $ret =
                    call(self.raw(), $index);
                function(self.raw() $(, $arg)*)
            }
        }
    )*};
}

/// 参照を一つ持つ `AMFInterface`。drop すると `Release`。
#[derive(Debug)]
struct Object(NonNull<c_void>);

impl Object {
    /// # Safety
    /// `raw` は null か、参照を一つ持っている生きた `AMFInterface`。
    unsafe fn from_raw(raw: *mut c_void) -> Option<Self> {
        NonNull::new(raw).map(Self)
    }

    fn raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    slots! {
        fn acquire() -> i32 = interface::ACQUIRE;
        fn release() -> i32 = interface::RELEASE;
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        self.release();
    }
}

/// 作成する関数の結果。失敗なら書き込まれた物は受け取らない。
///
/// # Safety
/// `raw` は null か、今の呼び出しが参照を一つ付けて書き込んだ物。
unsafe fn made(status: Status, raw: *mut c_void) -> Result<Object, Status> {
    if status != AMF_OK {
        return Err(status);
    }
    // SAFETY: 呼び出し側が保証するとおり。
    unsafe { Object::from_raw(raw) }.ok_or(status)
}

/// `AMFInit` が返すファクトリー。`AMFInterface` を継がず、解放する関数も無い。
#[derive(Debug, Clone, Copy)]
pub struct Factory(NonNull<c_void>);

// SAFETY: ファクトリーは AMF の仕様どおりスレッドセーフ。
unsafe impl Send for Factory {}
// SAFETY: 同上。
unsafe impl Sync for Factory {}

impl Factory {
    /// # Safety
    /// `raw` は null か、`AMFInit` が返し、プロセスの終わりまで生きるファクトリー。
    pub unsafe fn from_raw(raw: *mut c_void) -> Option<Self> {
        NonNull::new(raw).map(Self)
    }

    fn raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    slots! {
        @unsafe fn create_context_raw(out: *mut *mut c_void) -> Status = factory::CREATE_CONTEXT;
        @unsafe fn create_component_raw(context: *mut c_void, id: *const u16, out: *mut *mut c_void) -> Status = factory::CREATE_COMPONENT;
    }

    pub fn create_context(&self) -> Result<Context, Status> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: 出力は生きたローカル変数で、書き込まれた物は参照を一つ持つ。
        unsafe { made(self.create_context_raw(&mut raw), raw) }.map(Context)
    }

    pub fn create_component(&self, context: &Context, name: &str) -> Result<Component, Status> {
        let id = wide(name);
        let mut raw = std::ptr::null_mut();
        // SAFETY: コンテキストは生きていて、名前は NUL で終わる。出力は生きたローカル変数で参照を一つ持つ。
        unsafe {
            made(
                self.create_component_raw(context.raw(), id.as_ptr(), &mut raw),
                raw,
            )
        }
        .map(Component)
    }
}

/// `AMFContext`。
#[derive(Debug)]
pub struct Context(Object);

impl Context {
    fn raw(&self) -> *mut c_void {
        self.0.raw()
    }

    slots! {
        pub fn terminate() -> Status = context::TERMINATE;
        @unsafe fn init_dx11_raw(device: *mut c_void, version: i32) -> Status = context::INIT_DX11;
        @unsafe fn alloc_buffer_raw(memory: i32, size: usize, out: *mut *mut c_void) -> Status = context::ALLOC_BUFFER;
    }

    /// `device` は生きた `ID3D11Device` か、無ければ既定のデバイス。
    pub fn init_dx11(&self, device: Option<&ID3D11Device>) -> Status {
        let pointer = device.map_or(std::ptr::null_mut(), |device| device.as_raw());
        // SAFETY: デバイスは借りている間生きているか null。
        unsafe { self.init_dx11_raw(pointer, AMF_DX11_0) }
    }

    /// メインメモリのバッファを作って `data` をコピーする。アドレスの無いバッファは `Err(None)`。
    pub fn buffer(&self, data: &[u8]) -> Result<Buffer, Option<Status>> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: 出力は生きたローカル変数で、書き込まれた物は参照を一つ持つ。
        let made = unsafe {
            made(
                self.alloc_buffer_raw(AMF_MEMORY_HOST, data.len(), &mut raw),
                raw,
            )
        };
        let buffer = Buffer(made.map_err(Some)?);
        let into = buffer.native().cast::<u8>();
        if into.is_null() {
            return Err(None);
        }
        // SAFETY: メインメモリのバッファで、大きさは要求したとおり。重ならない。
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), into, data.len()) };
        Ok(buffer)
    }
}

/// `AMFComponent`。
#[derive(Debug)]
pub struct Component(Object);

impl Component {
    fn raw(&self) -> *mut c_void {
        self.0.raw()
    }

    slots! {
        pub fn init(format: i32, width: i32, height: i32) -> Status = component::INIT;
        pub fn terminate() -> Status = component::TERMINATE;
        pub fn drain() -> Status = component::DRAIN;
        pub fn flush() -> Status = component::FLUSH;
        @unsafe fn set_property_raw(name: *const u16, value: Variant) -> Status = interface::SET_PROPERTY;
        @unsafe fn submit_input_raw(data: *mut c_void) -> Status = component::SUBMIT_INPUT;
        @unsafe fn query_output_raw(out: *mut *mut c_void) -> Status = component::QUERY_OUTPUT;
        @unsafe fn get_caps_raw(out: *mut *mut c_void) -> Status = component::GET_CAPS;
    }

    pub fn set(&self, name: &str, value: Variant) -> Status {
        let name = wide(name);
        // SAFETY: 名前は NUL で終わり、呼び出しの間生きている。
        unsafe { self.set_property_raw(name.as_ptr(), value) }
    }

    pub fn submit(&self, buffer: &Buffer) -> Status {
        // SAFETY: バッファは生きている。コンポーネントは要るなら自分の参照を取る。
        unsafe { self.submit_input_raw(buffer.raw()) }
    }

    /// 出てきたフレーム。デコーダーが出すのは `AMFSurface`。
    pub fn query_output(&self) -> (Status, Option<Surface>) {
        let mut raw = std::ptr::null_mut();
        // SAFETY: 出力は生きたローカル変数で、書き込まれた物は参照を一つ持つ。
        unsafe {
            let status = self.query_output_raw(&mut raw);
            (status, Object::from_raw(raw).map(Surface))
        }
    }

    /// コンポーネントがどこでデコードするか。
    pub fn acceleration(&self) -> Result<i32, Status> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: 出力は生きたローカル変数で、書き込まれた物は参照を一つ持つ。
        let caps = Caps(unsafe { made(self.get_caps_raw(&mut raw), raw) }?);
        Ok(caps.acceleration_type())
    }
}

/// `AMFCaps`。
struct Caps(Object);

impl Caps {
    fn raw(&self) -> *mut c_void {
        self.0.raw()
    }

    slots! {
        fn acceleration_type() -> i32 = caps::GET_ACCELERATION_TYPE;
    }
}

/// `AMFBuffer`。
#[derive(Debug)]
pub struct Buffer(Object);

impl Buffer {
    fn raw(&self) -> *mut c_void {
        self.0.raw()
    }

    slots! {
        pub fn set_pts(time: i64) -> () = data::SET_PTS;
        fn native() -> *mut c_void = buffer::GET_NATIVE;
    }
}

/// `AMFSurface`。
#[derive(Debug)]
pub struct Surface(Object);

impl Surface {
    fn raw(&self) -> *mut c_void {
        self.0.raw()
    }

    /// 参照をもう一つ取る。
    pub fn share(&self) -> Self {
        self.0.acquire();
        Self(Object(self.0.0))
    }

    slots! {
        pub fn memory_type() -> i32 = data::GET_MEMORY_TYPE;
        pub fn convert(memory: i32) -> Status = data::CONVERT;
        pub fn pts() -> i64 = data::GET_PTS;
        pub fn format() -> i32 = surface::GET_FORMAT;
        pub fn planes_count() -> usize = surface::GET_PLANES_COUNT;
        fn plane_at_raw(index: usize) -> *mut c_void = surface::GET_PLANE_AT;
    }

    /// プレーン。参照は数えず、サーフェスより長く持たない。
    pub fn plane(&self, index: usize) -> Option<Plane<'_>> {
        let raw = NonNull::new(self.plane_at_raw(index))?;
        Some(Plane {
            raw,
            _surface: std::marker::PhantomData,
        })
    }
}

/// `AMFPlane`。
pub struct Plane<'a> {
    raw: NonNull<c_void>,
    _surface: std::marker::PhantomData<&'a Surface>,
}

impl Plane<'_> {
    fn raw(&self) -> *mut c_void {
        self.raw.as_ptr()
    }

    slots! {
        /// メインメモリなら最初の画素、DX11 なら `ID3D11Texture2D*`。
        pub fn native() -> *mut c_void = plane::GET_NATIVE;
        pub fn h_pitch() -> i32 = plane::GET_H_PITCH;
        pub fn v_pitch() -> i32 = plane::GET_V_PITCH;
    }
}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub const DECODER_MPEG2: &str = "AMFVideoDecoderUVD_MPEG2";
pub const DECODER_H264: &str = "AMFVideoDecoderUVD_H264_AVC";
pub const DECODER_HEVC: &str = "AMFVideoDecoderHW_H265_HEVC";
/// 8 ビットのコンポーネントに P010 で `Init` するとクラッシュするドライバーがある。
pub const DECODER_HEVC_MAIN10: &str = "AMFVideoDecoderHW_H265_MAIN10";

pub const TIMESTAMP_MODE: &str = "TimestampMode";
pub const SURFACE_CPU: &str = "SurfaceCpu";
pub const SURFACE_POOL_SIZE: &str = "SurfacePoolSize";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variant_is_twenty_four_bytes() {
        assert_eq!(size_of::<Variant>(), 24);
        assert_eq!(std::mem::offset_of!(Variant, value), 8);
    }

    #[test]
    fn the_version_packs_as_the_header_does() {
        assert_eq!(HEADER_VERSION, 0x0001_0005_0002_0000);
    }
}
