// Copyright 2026 Till0196
// SPDX-License-Identifier: Apache-2.0

//! Hands coded sound (AAC as IEC 61937) to a receiver without decoding it.
//! The OS only opens the link with the non-audio bit set; the wrapping is ours.
//! macOS は無い：CoreAudio は AAC の IEC 61937 を通せない。

use std::fmt;

#[cfg(all(feature = "passthrough", target_os = "linux"))]
pub mod alsa;
#[cfg(all(feature = "passthrough", target_os = "android"))]
pub mod audiotrack;
#[cfg(all(feature = "passthrough", target_os = "android"))]
mod jni;
#[cfg(all(feature = "passthrough", target_os = "linux"))]
pub mod pipewire;
#[cfg(feature = "passthrough")]
mod pump;
#[cfg(all(feature = "passthrough", windows))]
pub mod wasapi;

/// `Unavailable` is not a fault: no receiver attached is the ordinary case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unavailable(String),
    Unsupported(&'static str),
    Lost(String),
}

impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(what) => write!(out, "{what}"),
            Self::Unsupported(what) => write!(out, "{what}"),
            Self::Lost(what) => write!(out, "{what}"),
        }
    }
}

pub trait Link: Send {
    /// Takes one wrapped burst and gives back how many IEC 60958 frames were taken.
    /// All or nothing: half a burst on the wire makes the receiver lose lock.
    fn write(&mut self, burst: &[u8]) -> Result<usize, Error>;

    fn played(&self) -> u64;

    fn queued(&self) -> usize;

    fn rate(&self) -> u32;

    fn name(&self) -> &'static str;
}

/// No fallback to decoding: quietly playing something else hides which one is heard.
pub fn open(rate: u32) -> Result<Box<dyn Link>, Error> {
    open_on(rate, None)
}

/// `None` is the default device, which may be headphones with no passthrough.
pub fn open_on(rate: u32, on: Option<&str>) -> Result<Box<dyn Link>, Error> {
    platform(rate, on)
}

/// PipeWire first: where it runs it holds the device, and ALSA would find it busy.
#[cfg(all(feature = "passthrough", target_os = "linux"))]
fn platform(rate: u32, on: Option<&str>) -> Result<Box<dyn Link>, Error> {
    let first = match pipewire::PipeWire::open(rate, on) {
        Ok(link) => return Ok(Box::new(link)),
        Err(error) => error,
    };
    match alsa::Alsa::open(rate, on) {
        Ok(link) => Ok(Box::new(link)),
        Err(second) => Err(Error::Unavailable(format!(
            "PipeWire: {first}; ALSA: {second}"
        ))),
    }
}

#[cfg(all(feature = "passthrough", windows))]
fn platform(rate: u32, on: Option<&str>) -> Result<Box<dyn Link>, Error> {
    Ok(Box::new(wasapi::Wasapi::open(rate, on)?))
}

#[cfg(all(feature = "passthrough", target_os = "android"))]
fn platform(rate: u32, on: Option<&str>) -> Result<Box<dyn Link>, Error> {
    let _ = on;
    Ok(Box::new(audiotrack::AudioTrack::open(rate)?))
}

#[cfg(not(all(
    feature = "passthrough",
    any(target_os = "linux", windows, target_os = "android")
)))]
fn platform(rate: u32, on: Option<&str>) -> Result<Box<dyn Link>, Error> {
    let _ = (rate, on);
    Err(Error::Unavailable(String::from(
        "このビルドはパススルーに対応していない",
    )))
}
